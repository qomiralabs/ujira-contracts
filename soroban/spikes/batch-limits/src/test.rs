// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Runs the prototype as compiled wasm, because the native build meters host
//! functions only and leaves out the VM's own instructions, which are most of
//! the cost of a batch. Every number printed here is for the wasm.

extern crate std;

use ed25519_dalek::{Signer, SigningKey};
use serde_json::Value;
use sha2::{Digest, Sha256};
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
use std::path::Path;
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
    /// A classic account with a trustline, paid through a muxed `M` address
    /// with a 64-bit id. The tag adds `to_muxed_id` to every SAC transfer
    /// event and widens every leg in the mandate's own events, so this is the
    /// event-size worst case.
    Tagged,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub op: &'static str,
    pub mode: u32,
    /// Legs per item, or the whole plan when items differ.
    pub legs: String,
    pub fee: String,
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
    /// The whole envelope, estimated: the auth entry repeats the call and its
    /// arguments, so they count twice, plus about 100 bytes per footprint key
    /// and 400 for the rest. Calibrated on testnet, where a 16 item, 4-leg
    /// request had a 47,016 byte envelope and this gives 46,976.
    pub fn tx_size_estimate(&self) -> u32 {
        2 * self.args_bytes + 100 * self.footprint() + 400
    }

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
                "tx_size",
                f64::from(self.tx_size_estimate()) / f64::from(Limits::TX_SIZE_BYTES),
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

/// A key whose seed is the SHA-256 of a public label, so nothing here can ever
/// be a real key and anyone can rebuild it.
fn test_key(label: &str) -> SigningKey {
    SigningKey::from_bytes(&Sha256::digest(label.as_bytes()).into())
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
    let key = test_key("UJIRA/TEST-ONLY/S18/attester-ed25519");
    let config = Config {
        principal,
        asset: sac.address(),
        fee_address,
        agents: Vec::from_array(&env, [agent.clone()]),
        recipient_mode: mode,
        attester: Bytes::from_array(&env, key.verifying_key().as_bytes()),
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
    let message = w
        .client()
        .message(recipient_ref, destination, &valid_until, &0);
    let mut raw = std::vec![0u8; message.len() as usize];
    message.copy_into_slice(&mut raw);
    Attestation {
        valid_until,
        policy_epoch: 0,
        signature: Bytes::from_array(&w.env, &w.key.sign(&raw).to_bytes()),
    }
}

fn muxed(env: &Env, account: &Address, id: u64) -> MuxedAddress {
    let key = match ScAddress::from(account) {
        ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(k))) => k,
        other => panic!("not an account: {other:?}"),
    };
    let sc = xdr::ScVal::Address(ScAddress::MuxedAccount(xdr::MuxedEd25519Account {
        id,
        ed25519: key,
    }));
    MuxedAddress::try_from_val(env, &sc).unwrap()
}

/// One request per plan entry: (legs, whether it carries a fee).
fn build(w: &mut World, plan: &[(u32, bool)], dest: Dest, batch: u32) -> StdVec<PayoutRequest> {
    let cfg_mode = w.mode;
    let mut reqs = StdVec::new();
    let mut allow = Vec::new(&w.env);
    for (i, &(legs, fee)) in (0u32..).zip(plan.iter()) {
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
                Dest::Contract | Dest::Tagged => Address::generate(&w.env),
            };
            let destination = match dest {
                Dest::Tagged => {
                    let a = account_address(&w.env, w.next_account);
                    w.next_account += 1;
                    create_receiver(&w.env, &a, &w.asset);
                    // Every id is a full 8 bytes in XDR, whatever its value.
                    muxed(&w.env, &a, u64::MAX - u64::from(w.next_account))
                }
                _ => MuxedAddress::from(address),
            };
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

fn row(w: &World, label: &Label, op: &'static str, args_bytes: u32, return_bytes: u32) -> Row {
    let r = w.env.cost_estimate().resources();
    Row {
        op,
        mode: label.mode,
        legs: label.legs.clone(),
        fee: label.fee.clone(),
        dest: label.dest,
        items: label.items,
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

/// How a measured batch is described in the table.
struct Label {
    mode: u32,
    legs: String,
    fee: String,
    dest: Dest,
    items: u32,
}

fn xdr_len<T: soroban_sdk::IntoVal<Env, soroban_sdk::Val> + Clone>(env: &Env, v: &T) -> u32 {
    use soroban_sdk::xdr::ToXdr;
    let val: soroban_sdk::Val = v.clone().into_val(env);
    val.to_xdr(env).len()
}

/// Requests a batch, checks every item landed approved, executes the same
/// batch, and returns both measurements.
pub fn measure(mode: u32, legs: u32, fee: bool, dest: Dest, items: u32) -> (Row, Row) {
    let plan: StdVec<(u32, bool)> = (0..items).map(|_| (legs, fee)).collect();
    measure_plan(mode, &plan, dest)
}

/// The same for a batch whose items differ in legs or fee.
pub fn measure_plan(mode: u32, plan: &[(u32, bool)], dest: Dest) -> (Row, Row) {
    let items = u32::try_from(plan.len()).unwrap();
    let uniform = plan.iter().all(|p| *p == plan[0]);
    let legs_total: u32 = plan.iter().map(|p| p.0).sum();
    let fees = u32::try_from(plan.iter().filter(|p| p.1).count()).unwrap();
    let label = Label {
        mode,
        legs: if uniform {
            format!("{}", plan[0].0)
        } else {
            format!("{legs_total} in all")
        },
        fee: match (uniform, fees) {
            (true, 0) => String::from("no"),
            (true, _) => String::from("yes"),
            (false, n) => format!("{n} of {items}"),
        },
        dest,
        items,
    };
    let mut w = world(mode);
    let reqs = build(&mut w, plan, dest, 1);
    let batch = Vec::from_slice(&w.env, &reqs);
    let args = xdr_len(&w.env, &batch);
    let results = w.client().request_payouts(&w.agent, &batch);
    let ret = xdr_len(&w.env, &results);
    let request = row(&w, &label, "request_payouts", args, ret);
    for r in results.iter() {
        assert_eq!((r.code, r.status), (0, STATUS_APPROVED), "item rejected");
    }

    let ids: Vec<BytesN<32>> = Vec::from_iter(&w.env, reqs.iter().map(|r| r.payout_id.clone()));
    let args = xdr_len(&w.env, &ids);
    w.client().execute_batch(&w.agent, &ids);
    // A unit return is ScVal::Void, 4 bytes of XDR.
    let execute = row(&w, &label, "execute_batch", args, 4);
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
        r.fee,
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
    // Tagged destinations, the event-size worst case.
    for legs in [1, 2, 4] {
        cases.push((MODE_ALLOWLIST, legs, true, Dest::Tagged));
    }
    cases.push((MODE_ATTESTED, 4, true, Dest::Tagged));
    cases.push((MODE_ALLOWLIST, 2, false, Dest::Tagged));

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
/// fee). Tagged legs are the event-size worst case: 19 transfers of them reach
/// 69.3 percent of the events limit in the env, and the network adds about
/// half a point, so 18 is the most that keeps 30 percent headroom.
const MAX_BATCH: u32 = 10;
const MAX_BATCH_TRANSFERS: u32 = 18;

fn assert_headroom(r: &Row) {
    for (name, share) in r.shares() {
        assert!(
            share <= 1.0 - HEADROOM,
            "{} {} legs {} items {:?}: {name} at {:.1}% of the limit",
            r.op,
            r.legs,
            r.items,
            r.dest,
            share * 100.0
        );
    }
}

/// Legs and fee, one entry per item.
type Plan = StdVec<(u32, bool)>;

fn transfers(plan: &[(u32, bool)]) -> u32 {
    plan.iter().map(|&(legs, fee)| legs + u32::from(fee)).sum()
}

#[test]
fn recommended_limits_keep_headroom() {
    let fours = |n: usize, fee: bool| -> Plan { (0..n).map(|_| (4, fee)).collect() };
    let cases: StdVec<(u32, Plan, Dest)> = StdVec::from([
        // Most event bytes for the transfer budget: every transfer a leg, on
        // as many items as allowed, tagged and untagged.
        (
            MODE_ALLOWLIST,
            worst_plan(MAX_BATCH_TRANSFERS, MAX_BATCH),
            Dest::Tagged,
        ),
        (
            MODE_ALLOWLIST,
            worst_plan(MAX_BATCH_TRANSFERS, MAX_BATCH),
            Dest::Account,
        ),
        // Four tagged legs with fees, up to the transfer budget.
        (
            MODE_BOTH,
            StdVec::from([(4, true), (4, true), (4, false), (4, false)]),
            Dest::Tagged,
        ),
        (MODE_BOTH, fours(3, true), Dest::Tagged),
        // Most items with a fee each, inside the transfer budget.
        (
            MODE_ALLOWLIST,
            (0..9).map(|_| (1, true)).collect(),
            Dest::Tagged,
        ),
        // Largest requests: the full item count at four legs. Their execute
        // would exceed the transfer budget, so only the request is checked.
        (MODE_BOTH, fours(10, false), Dest::Tagged),
        (MODE_ATTESTED, fours(10, true), Dest::Tagged),
        (MODE_BOTH, fours(10, false), Dest::Account),
    ]);
    println!("{}", header());
    for (mode, plan, dest) in cases {
        assert!(plan.len() <= MAX_BATCH as usize);
        let (rq, ex) = measure_plan(mode, &plan, dest);
        println!("{}", line(&rq));
        assert_headroom(&rq);
        if transfers(&plan) <= MAX_BATCH_TRANSFERS {
            println!("{}", line(&ex));
            assert_headroom(&ex);
        }
    }
}

/// The plan that spends a transfer budget on the most event bytes: as many
/// items as `MAX_BATCH` allows, every transfer a tagged leg, no fee (a leg
/// costs more event bytes than a fee transfer, because it also appears in
/// the mandate's own events).
fn worst_plan(transfers: u32, items: u32) -> StdVec<(u32, bool)> {
    let items = items.min(transfers);
    (0..items)
        .map(|i| (transfers / items + u32::from(i < transfers % items), false))
        .collect()
}

/// Sweeps the transfer budget around the boundary so the table shows where
/// 70 percent of the events limit falls for the worst plan.
#[test]
fn transfer_cap_sweep() {
    println!("{}", header());
    for transfers in 16..=20 {
        for dest in [Dest::Account, Dest::Tagged] {
            let (_, ex) = measure_plan(MODE_ALLOWLIST, &worst_plan(transfers, MAX_BATCH), dest);
            println!("{}", line(&ex));
        }
    }
    // Fewer items for the same transfers only removes per-item bytes, but
    // check the other end too: one item per transfer, up to MAX_BATCH.
    for dest in [Dest::Account, Dest::Tagged] {
        let (_, ex) = measure_plan(MODE_ALLOWLIST, &worst_plan(MAX_BATCH, MAX_BATCH), dest);
        println!("{}", line(&ex));
    }
}

fn unhex(h: &str) -> StdVec<u8> {
    let h = h.strip_prefix("0x").unwrap_or(h);
    (0..h.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
        .collect()
}

fn array32(h: &str) -> [u8; 32] {
    unhex(h).try_into().unwrap()
}

/// The prototype builds the same bytes as the parity spike's reference signer,
/// for one leg of each kind of destination. It runs at the fixture's own
/// address and network, because both are part of the message.
#[test]
fn message_matches_the_parity_fixture_for_every_kind_of_destination() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../spikes/attestation-parity/fixture.json");
    let fixture: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    for name in [
        "valid_untagged",
        "valid_contract_destination",
        "valid_tagged",
    ] {
        let case = cases
            .iter()
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("the fixture has no case {name}"));
        let env = Env::new_with_config(soroban_sdk::testutils::EnvTestConfig {
            capture_snapshot_at_drop: false,
        });
        env.mock_all_auths();
        env.ledger()
            .set_network_id(array32(case["network_id"].as_str().unwrap()));
        let admin = Address::generate(&env);
        let asset = env.register_stellar_asset_contract_v2(admin).address();
        let key = test_key("UJIRA/TEST-ONLY/S18/attester-ed25519");
        let config = Config {
            principal: Address::generate(&env),
            asset,
            fee_address: Address::generate(&env),
            agents: Vec::from_array(&env, [Address::generate(&env)]),
            recipient_mode: MODE_ATTESTED,
            attester: Bytes::from_array(&env, key.verifying_key().as_bytes()),
            country_policy_hash: BytesN::from_array(
                &env,
                &array32(case["country_policy_hash"].as_str().unwrap()),
            ),
            max_fee_bps: 100,
            total_budget: 1_000_000,
            per_payment_cap: 1_000,
            per_recipient_daily_cap: 1_000,
            approval_threshold: 1_001,
            payout_ttl_seconds: 86_400,
            expires_at: NOW + 86_400,
        };
        let mandate = soroban_sdk::String::from_str(&env, case["mandate"].as_str().unwrap());
        let mandate = Address::from_string(&mandate);
        env.register_at(&mandate, proto::WASM, (config,));

        let leg = &case["legs"][0];
        let base = soroban_sdk::String::from_str(&env, leg["destination"].as_str().unwrap());
        let destination: MuxedAddress = if leg["has_tag"].as_bool().unwrap() {
            let raw = unhex(leg["destination_xdr"].as_str().unwrap());
            let account = ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(
                raw[raw.len() - 32..].try_into().unwrap(),
            ))));
            muxed(
                &env,
                &Address::try_from_val(&env, &account).unwrap(),
                leg["tag"].as_str().unwrap().parse().unwrap(),
            )
        } else {
            MuxedAddress::from(Address::from_string(&base))
        };
        let attestation = &case["attestations"][0];
        let got = proto::Client::new(&env, &mandate).message(
            &BytesN::from_array(&env, &array32(case["recipient_ref"].as_str().unwrap())),
            &destination,
            &attestation["valid_until"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap(),
            &attestation["policy_epoch"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap(),
        );
        assert_eq!(
            got,
            Bytes::from_slice(
                &env,
                &unhex(attestation["checked"]["message"].as_str().unwrap())
            ),
            "{name}"
        );
    }
}

/// The epoch is checked before the signature, so an attestation with both
/// wrong gets a code for its own item instead of trapping the whole batch.
#[test]
fn a_stale_policy_epoch_wins_over_a_bad_signature() {
    let mut w = world(MODE_ATTESTED);
    let mut reqs = build(&mut w, &[(1, false)], Dest::Account, 1);
    let leg_attestation = reqs[0].attestations.get(0).unwrap();
    reqs[0].attestations = Vec::from_array(
        &w.env,
        [Attestation {
            valid_until: leg_attestation.valid_until,
            policy_epoch: 1,
            signature: Bytes::from_array(&w.env, &[7u8; 64]),
        }],
    );
    let batch = Vec::from_slice(&w.env, &reqs);
    let results = w.client().request_payouts(&w.agent, &batch);
    assert_eq!(results.get(0).unwrap().code, 53);
}

/// A key of the wrong length, or 32 zero bytes, can never verify anything, so
/// the constructor refuses it.
#[test]
fn the_constructor_refuses_a_malformed_attester_key() {
    for bad in [
        StdVec::from([1u8; 31]),
        StdVec::from([1u8; 33]),
        StdVec::from([0u8; 32]),
    ] {
        let env = Env::new_with_config(soroban_sdk::testutils::EnvTestConfig {
            capture_snapshot_at_drop: false,
        });
        let asset = env
            .register_stellar_asset_contract_v2(Address::generate(&env))
            .address();
        let config = Config {
            principal: Address::generate(&env),
            asset,
            fee_address: Address::generate(&env),
            agents: Vec::from_array(&env, [Address::generate(&env)]),
            recipient_mode: MODE_ATTESTED,
            attester: Bytes::from_slice(&env, &bad),
            country_policy_hash: BytesN::from_array(&env, &[3; 32]),
            max_fee_bps: 100,
            total_budget: 1_000,
            per_payment_cap: 1_000,
            per_recipient_daily_cap: 1_000,
            approval_threshold: 1_001,
            payout_ttl_seconds: 86_400,
            expires_at: NOW + 86_400,
        };
        let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            env.register(proto::WASM, (config,))
        }));
        assert!(refused.is_err(), "the key {bad:?} was accepted");
    }
}
