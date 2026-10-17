// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Runs the prototype as compiled wasm, because the native build meters host
//! functions only and leaves out the VM's own instructions, which are most of
//! the cost of a batch. Every number printed here is for the wasm.

extern crate std;

use k256::ecdsa::SigningKey;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token::StellarAssetClient,
    xdr::{
        self, AccountEntry, AccountEntryExt, AccountId, LedgerEntry, LedgerEntryData,
        LedgerEntryExt, LedgerKey, LedgerKeyAccount, LedgerKeyTrustLine, PublicKey, ScAddress,
        SequenceNumber, Thresholds, TrustLineAsset, TrustLineEntry, TrustLineEntryExt, Uint256,
    },
    Address, Bytes, BytesN, Env, MuxedAddress, TryFromVal, Vec,
};
use std::{format, println, rc::Rc, string::String, vec::Vec as StdVec};

mod proto {
    soroban_sdk::contractimport!(file = "fixtures/batch_limits_proto.wasm");
}
use proto::{AllowEntry, Attestation, Config, Leg, PayoutRequest};

pub const MODE_ALLOWLIST: u32 = 1;
pub const MODE_ATTESTED: u32 = 2;
pub const MODE_BOTH: u32 = 3;
const STATUS_APPROVED: u32 = 2;
const NOW: u64 = 1_800_000_000;
const LEG_AMOUNT: i128 = 1_000_000;

/// Per-transaction limits read from `stellar network settings` on testnet and
/// on mainnet, ledger 5243815 and 64991374, 2026-10-17. The two networks agree
/// on every per-transaction value.
pub struct Limits;
impl Limits {
    pub const INSTRUCTIONS: i64 = 400_000_000;
    pub const MEMORY: i64 = 41_943_040;
    pub const DISK_READ_ENTRIES: u32 = 200;
    pub const DISK_READ_BYTES: u32 = 200_000;
    pub const WRITE_ENTRIES: u32 = 200;
    pub const WRITE_BYTES: u32 = 132_096;
    pub const FOOTPRINT_ENTRIES: u32 = 400;
    pub const EVENTS_BYTES: u32 = 16_384;
    pub const TX_SIZE_BYTES: u32 = 132_096;
}

/// The issue asks for at least 30 percent headroom on every resource.
pub const HEADROOM: f64 = 0.30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dest {
    /// A classic account with a trustline, the usual Stellar wallet.
    Account,
    /// A contract address, whose balance entry the SAC creates on first credit.
    Contract,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub op: &'static str,
    pub mode: u32,
    pub legs: u32,
    pub fee: bool,
    pub dest: Dest,
    pub items: u32,
    pub instructions: i64,
    pub mem: i64,
    pub disk_read_entries: u32,
    pub memory_read_entries: u32,
    pub write_entries: u32,
    pub disk_read_bytes: u32,
    pub write_bytes: u32,
    pub events_bytes: u32,
    /// The network charges the return value against the events limit too.
    pub return_bytes: u32,
    pub args_bytes: u32,
}

impl Row {
    /// Distinct keys the transaction must declare. Every key accessed is
    /// counted once, as a disk or an in-memory read.
    pub fn footprint(&self) -> u32 {
        self.disk_read_entries + self.memory_read_entries
    }

    /// Each resource as a share of its limit, with its name.
    pub fn shares(&self) -> [(&'static str, f64); 9] {
        [
            (
                "instructions",
                self.instructions as f64 / Limits::INSTRUCTIONS as f64,
            ),
            ("memory", self.mem as f64 / Limits::MEMORY as f64),
            (
                "disk_read_entries",
                f64::from(self.disk_read_entries) / f64::from(Limits::DISK_READ_ENTRIES),
            ),
            (
                "disk_read_bytes",
                f64::from(self.disk_read_bytes) / f64::from(Limits::DISK_READ_BYTES),
            ),
            (
                "write_entries",
                f64::from(self.write_entries) / f64::from(Limits::WRITE_ENTRIES),
            ),
            (
                "write_bytes",
                f64::from(self.write_bytes) / f64::from(Limits::WRITE_BYTES),
            ),
            (
                "footprint",
                f64::from(self.footprint()) / f64::from(Limits::FOOTPRINT_ENTRIES),
            ),
            (
                "events",
                f64::from(self.events_bytes + self.return_bytes) / f64::from(Limits::EVENTS_BYTES),
            ),
            (
                "tx_args",
                f64::from(self.args_bytes) / f64::from(Limits::TX_SIZE_BYTES),
            ),
        ]
    }

    pub fn binding(&self) -> (&'static str, f64) {
        let mut best = ("", 0.0);
        for (name, share) in self.shares() {
            if share > best.1 {
                best = (name, share);
            }
        }
        best
    }
}

pub fn mode_name(mode: u32) -> &'static str {
    match mode {
        MODE_ALLOWLIST => "allowlist",
        MODE_ATTESTED => "attested",
        _ => "both",
    }
}

struct World {
    env: Env,
    mandate: Address,
    mode: u32,
    asset: xdr::Asset,
    key: SigningKey,
    agent: Address,
    next_account: u32,
}

fn account_address(env: &Env, n: u32) -> Address {
    let mut raw = [0x5au8; 32];
    raw[..4].copy_from_slice(&n.to_be_bytes());
    let id = AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(raw)));
    Address::try_from_val(env, &ScAddress::Account(id)).unwrap()
}

fn add_entry(env: &Env, key: LedgerKey, data: LedgerEntryData) {
    let entry = LedgerEntry {
        data,
        last_modified_ledger_seq: 0,
        ext: LedgerEntryExt::V0,
    };
    env.host()
        .add_ledger_entry(&Rc::new(key), &Rc::new(entry), None)
        .unwrap();
}

/// A funded classic account holding an authorized trustline for the asset.
fn create_receiver(env: &Env, address: &Address, asset: &xdr::Asset) {
    let account_id = match ScAddress::from(address) {
        ScAddress::Account(id) => id,
        other => panic!("not an account: {other:?}"),
    };
    add_entry(
        env,
        LedgerKey::Account(LedgerKeyAccount {
            account_id: account_id.clone(),
        }),
        LedgerEntryData::Account(AccountEntry {
            account_id: account_id.clone(),
            balance: 100_000_000,
            flags: 0,
            home_domain: Default::default(),
            inflation_dest: None,
            num_sub_entries: 1,
            seq_num: SequenceNumber(0),
            thresholds: Thresholds([1; 4]),
            signers: Default::default(),
            ext: AccountEntryExt::V0,
        }),
    );
    let line = match asset.clone() {
        xdr::Asset::CreditAlphanum4(a) => TrustLineAsset::CreditAlphanum4(a),
        xdr::Asset::CreditAlphanum12(a) => TrustLineAsset::CreditAlphanum12(a),
        xdr::Asset::Native => TrustLineAsset::Native,
    };
    add_entry(
        env,
        LedgerKey::Trustline(LedgerKeyTrustLine {
            account_id: account_id.clone(),
            asset: line.clone(),
        }),
        LedgerEntryData::Trustline(TrustLineEntry {
            account_id,
            asset: line,
            balance: 0,
            limit: i64::MAX,
            flags: xdr::TrustLineFlags::AuthorizedFlag as u32,
            ext: TrustLineEntryExt::V0,
        }),
    );
}

fn attester_key(key: &SigningKey) -> [u8; 65] {
    let point = key.verifying_key().to_encoded_point(false);
    let mut out = [0u8; 65];
    out.copy_from_slice(point.as_bytes());
    out
}

fn world(mode: u32) -> World {
    // These envs exist to be measured, not to be kept as regression fixtures,
    // and a snapshot of a 40-item batch runs to megabytes.
    let env = Env::new_with_config(soroban_sdk::testutils::EnvTestConfig {
        capture_snapshot_at_drop: false,
    });
    env.mock_all_auths();
    // Batches past the limits are measured on purpose; compare afterwards.
    env.cost_estimate().disable_resource_limits();
    // Testnet's archival settings, so rent and TTL work the way they do there.
    env.ledger().with_mut(|li| {
        li.timestamp = NOW;
        li.sequence_number = 1_000;
        li.min_temp_entry_ttl = 720;
        li.min_persistent_entry_ttl = 120_960;
        li.max_entry_ttl = 3_110_400;
    });
    let admin = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(admin);
    let asset = sac.asset();
    let principal = Address::generate(&env);
    let agent = Address::generate(&env);
    let fee_address = account_address(&env, 0);
    create_receiver(&env, &fee_address, &asset);
    let key = SigningKey::from_bytes(&[7u8; 32].into()).unwrap();
    let config = Config {
        principal,
        asset: sac.address(),
        fee_address,
        agents: Vec::from_array(&env, [agent.clone()]),
        recipient_mode: mode,
        attester: BytesN::from_array(&env, &attester_key(&key)),
        country_policy_hash: BytesN::from_array(&env, &[3; 32]),
        max_fee_bps: 100,
        total_budget: i128::MAX / 4,
        per_payment_cap: i128::MAX / 4,
        per_recipient_daily_cap: i128::MAX / 4,
        // Above every request, so every item is approved and executable.
        approval_threshold: i128::MAX / 4,
        payout_ttl_seconds: 86_400,
        expires_at: NOW + 30 * 86_400,
    };
    let mandate = env.register(proto::WASM, (config,));
    StellarAssetClient::new(&env, &sac.address()).mint(&mandate, &(i128::from(i64::MAX)));
    World {
        env,
        mandate,
        mode,
        asset,
        key,
        agent,
        next_account: 1,
    }
}

impl World {
    fn client(&self) -> proto::Client<'_> {
        proto::Client::new(&self.env, &self.mandate)
    }
}

fn bytes32(env: &Env, tag: u8, n: u32) -> BytesN<32> {
    let mut raw = [tag; 32];
    raw[28..].copy_from_slice(&n.to_be_bytes());
    BytesN::from_array(env, &raw)
}

fn sign(
    w: &World,
    recipient_ref: &BytesN<32>,
    destination: &MuxedAddress,
    valid_until: u64,
) -> Attestation {
    let digest = w
        .client()
        .digest(recipient_ref, destination, &valid_until)
        .to_array();
    let (mut sig, mut recid) = w.key.sign_prehash_recoverable(&digest).unwrap();
    if let Some(low) = sig.normalize_s() {
        sig = low;
        recid = k256::ecdsa::RecoveryId::from_byte(recid.to_byte() ^ 1).unwrap();
    }
    let mut raw = [0u8; 65];
    raw[..64].copy_from_slice(&sig.to_bytes());
    raw[64] = 27 + recid.to_byte();
    Attestation {
        valid_until,
        signature: Bytes::from_array(&w.env, &raw),
    }
}

fn build(
    w: &mut World,
    items: u32,
    legs: u32,
    fee: bool,
    dest: Dest,
    batch: u32,
) -> StdVec<PayoutRequest> {
    let cfg_mode = w.mode;
    let mut reqs = StdVec::new();
    let mut allow = Vec::new(&w.env);
    for i in 0..items {
        let n = batch * 1_000 + i;
        let recipient_ref = bytes32(&w.env, 0x11, n);
        let mut leg_vec = Vec::new(&w.env);
        let mut atts = Vec::new(&w.env);
        for _ in 0..legs {
            let address = match dest {
                Dest::Account => {
                    let a = account_address(&w.env, w.next_account);
                    w.next_account += 1;
                    create_receiver(&w.env, &a, &w.asset);
                    a
                }
                Dest::Contract => Address::generate(&w.env),
            };
            let destination = MuxedAddress::from(address);
            if cfg_mode != MODE_ATTESTED {
                allow.push_back(AllowEntry {
                    recipient_ref: recipient_ref.clone(),
                    destination: destination.clone(),
                });
            }
            if cfg_mode != MODE_ALLOWLIST {
                atts.push_back(sign(w, &recipient_ref, &destination, NOW + 3_600));
            }
            leg_vec.push_back(Leg {
                destination,
                amount: LEG_AMOUNT,
            });
        }
        let total = LEG_AMOUNT * i128::from(legs);
        reqs.push(PayoutRequest {
            payout_id: bytes32(&w.env, 0x22, n),
            recipient_ref,
            total,
            // The highest fee the 100 bps cap allows on this total.
            fee: if fee { total / 100 } else { 0 },
            legs: leg_vec,
            deadline: NOW + 600,
            task_ref: bytes32(&w.env, 0x33, n),
            attestations: atts,
        });
    }
    if !allow.is_empty() {
        w.client().allow_recipients(&allow);
    }
    reqs
}

#[allow(clippy::too_many_arguments)]
fn row(
    w: &World,
    op: &'static str,
    mode: u32,
    legs: u32,
    fee: bool,
    dest: Dest,
    items: u32,
    args_bytes: u32,
    return_bytes: u32,
) -> Row {
    let r = w.env.cost_estimate().resources();
    Row {
        op,
        mode,
        legs,
        fee,
        dest,
        items,
        instructions: r.instructions,
        mem: r.mem_bytes,
        disk_read_entries: r.disk_read_entries,
        memory_read_entries: r.memory_read_entries,
        write_entries: r.write_entries,
        disk_read_bytes: r.disk_read_bytes,
        write_bytes: r.write_bytes,
        events_bytes: r.contract_events_size_bytes,
        return_bytes,
        args_bytes,
    }
}

fn xdr_len<T: soroban_sdk::IntoVal<Env, soroban_sdk::Val> + Clone>(env: &Env, v: &T) -> u32 {
    use soroban_sdk::xdr::ToXdr;
    let val: soroban_sdk::Val = v.clone().into_val(env);
    val.to_xdr(env).len()
}

/// Requests a batch, checks every item landed approved, executes the same
/// batch, and returns both measurements.
pub fn measure(mode: u32, legs: u32, fee: bool, dest: Dest, items: u32) -> (Row, Row) {
    let mut w = world(mode);
    let reqs = build(&mut w, items, legs, fee, dest, 1);
    let batch = Vec::from_slice(&w.env, &reqs);
    let args = xdr_len(&w.env, &batch);
    let results = w.client().request_payouts(&w.agent, &batch);
    let ret = xdr_len(&w.env, &results);
    let request = row(
        &w,
        "request_payouts",
        mode,
        legs,
        fee,
        dest,
        items,
        args,
        ret,
    );
    for r in results.iter() {
        assert_eq!((r.code, r.status), (0, STATUS_APPROVED), "item rejected");
    }

    let ids: Vec<BytesN<32>> = Vec::from_iter(&w.env, reqs.iter().map(|r| r.payout_id.clone()));
    let args = xdr_len(&w.env, &ids);
    w.client().execute_batch(&w.agent, &ids);
    // A unit return is ScVal::Void, 4 bytes of XDR.
    let execute = row(&w, "execute_batch", mode, legs, fee, dest, items, args, 4);
    (request, execute)
}

pub fn header() -> String {
    String::from(
        "| op | mode | legs | fee | dest | items | instructions | memory | disk reads | mem reads | writes | read bytes | write bytes | event bytes | return bytes | arg bytes | footprint | binding | share |\n| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |",
    )
}

pub fn line(r: &Row) -> String {
    let (name, share) = r.binding();
    format!(
        "| {} | {} | {} | {} | {:?} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {:.1}% |",
        r.op,
        mode_name(r.mode),
        r.legs,
        if r.fee { "yes" } else { "no" },
        r.dest,
        r.items,
        r.instructions,
        r.mem,
        r.disk_read_entries,
        r.memory_read_entries,
        r.write_entries,
        r.disk_read_bytes,
        r.write_bytes,
        r.events_bytes,
        r.return_bytes,
        r.args_bytes,
        r.footprint(),
        name,
        share * 100.0
    )
}

/// Largest batch that keeps every resource at or under 70 percent of its
/// limit, from a straight line through the 1 and 40 item measurements.
/// Every resource measured here grows linearly with the item count.
pub fn max_items(one: &Row, forty: &Row) -> (u32, &'static str) {
    let a = one.shares();
    let b = forty.shares();
    let mut best = (u32::MAX, "");
    for i in 0..a.len() {
        let per_item = (b[i].1 - a[i].1) / 39.0;
        if per_item <= 0.0 {
            continue;
        }
        let fixed = a[i].1 - per_item;
        let n = ((1.0 - HEADROOM - fixed) / per_item).floor();
        let n = if n < 0.0 { 0 } else { n as u32 };
        if n < best.0 {
            best = (n, a[i].0);
        }
    }
    best
}

const SIZES: [u32; 4] = [1, 10, 20, 40];

#[test]
fn batch_table() {
    let mut cases: StdVec<(u32, u32, bool, Dest)> = StdVec::new();
    for mode in [MODE_ALLOWLIST, MODE_ATTESTED, MODE_BOTH] {
        for legs in [1, 2, 4] {
            cases.push((mode, legs, true, Dest::Account));
        }
    }
    cases.push((MODE_ALLOWLIST, 1, false, Dest::Account));
    cases.push((MODE_ATTESTED, 4, true, Dest::Contract));

    println!("{}", header());
    let mut limits_out = StdVec::new();
    for (mode, legs, fee, dest) in cases {
        let mut req_rows = StdVec::new();
        let mut exe_rows = StdVec::new();
        for n in SIZES {
            let (rq, ex) = measure(mode, legs, fee, dest, n);
            println!("{}", line(&rq));
            println!("{}", line(&ex));
            req_rows.push(rq);
            exe_rows.push(ex);
        }
        let (rn, rres) = max_items(&req_rows[0], &req_rows[3]);
        let (en, eres) = max_items(&exe_rows[0], &exe_rows[3]);
        limits_out.push(format!(
            "| {} | {} | {} | {:?} | {} ({}) | {} ({}) |",
            mode_name(mode),
            legs,
            if fee { "yes" } else { "no" },
            dest,
            rn,
            rres,
            en,
            eres
        ));
    }
    println!();
    println!("Largest batch with 30 percent headroom on every resource:");
    println!("| mode | legs | fee | dest | request_payouts | execute_batch |\n| --- | --- | --- | --- | --- | --- |");
    for l in limits_out {
        println!("{l}");
    }
}

/// The candidate limits: at most `MAX_BATCH` items per batch, and on execute
/// at most `MAX_BATCH_TRANSFERS` transfers (every leg, plus one per non-zero
/// fee). Each case below is a worst case for one resource under those limits.
const MAX_BATCH: u32 = 10;
const MAX_BATCH_TRANSFERS: u32 = 20;

fn assert_headroom(r: &Row) {
    for (name, share) in r.shares() {
        assert!(
            share <= 1.0 - HEADROOM,
            "{} {} legs {} items: {name} at {:.1}% of the limit",
            r.op,
            r.legs,
            r.items,
            share * 100.0
        );
    }
}

#[test]
fn recommended_limits_keep_headroom() {
    // (mode, legs, fee, items): transfers per execute = items * (legs + fee).
    let cases = [
        // Most event bytes per transfer budget: every transfer is a leg.
        (MODE_ALLOWLIST, 2, false, MAX_BATCH),
        // Most instructions: four attestations per item, on every item.
        (MODE_BOTH, 4, true, 4),
        // Largest request: the full item count at four legs.
        (MODE_BOTH, 4, false, MAX_BATCH),
        (MODE_ATTESTED, 4, true, MAX_BATCH),
        // Most items with a fee each.
        (MODE_ALLOWLIST, 1, true, MAX_BATCH),
    ];
    println!("{}", header());
    for (mode, legs, fee, items) in cases {
        let (rq, ex) = measure(mode, legs, fee, Dest::Account, items);
        println!("{}", line(&rq));
        assert_headroom(&rq);
        let transfers = items * (legs + u32::from(fee));
        if transfers <= MAX_BATCH_TRANSFERS {
            println!("{}", line(&ex));
            assert_headroom(&ex);
        }
    }
}
