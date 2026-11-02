// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The worst batches the production mandate accepts, run as compiled wasm
//! through the production factory, because the native build meters host
//! functions only and misses the VM's own instructions.
//!
//! The batch limits only protect the contract while these numbers hold. A
//! heavier event or an extra storage write must fail here rather than on
//! chain, so every assertion is in integers and rounds against the contract.

extern crate std;

use ed25519_dalek::{Signer, SigningKey};
use mandate_config::{
    Limits as MandateLimits, MandateConfig, MODE_ALLOWLIST, MODE_ATTESTED, MODE_BOTH,
};
use sha2::{Digest, Sha256};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger as _},
    token::StellarAssetClient,
    xdr::{
        self, AccountEntry, AccountEntryExt, AccountId, ContractEventBody, LedgerEntry,
        LedgerEntryData, LedgerEntryExt, LedgerKey, LedgerKeyAccount, LedgerKeyTrustLine,
        PublicKey, ScAddress, ScMapEntry, ScVal, SequenceNumber, Thresholds, TrustLineAsset,
        TrustLineEntry, TrustLineEntryExt, Uint256,
    },
    Address, Bytes, BytesN, Env, IntoVal, MuxedAddress, TryFromVal, Val, Vec,
};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::string::{String, ToString};
use std::sync::OnceLock;
use std::time::SystemTime;
use std::vec::Vec as StdVec;
use std::{format, println};
use ujira_factory::UjiraFactoryClient;
use ujira_mandate::execute::MAX_BATCH_TRANSFERS;
use ujira_mandate::payout::{MAX_BATCH, MAX_LEGS};
use ujira_mandate::types::{
    AllowEntry, Attestation, Leg, PayoutRequest, STATUS_APPROVED, STATUS_EXECUTED,
};
use ujira_mandate::UjiraMandateClient;

/// Per-transaction limits, read with `stellar network settings`. Testnet and
/// mainnet agreed on every value below when these were read. Re-read them and
/// re-run this suite on any protocol upgrade and before the spec is frozen.
const SETTINGS_READ: &str = "testnet ledger 5511789 and mainnet ledger 65259360, protocol 29, 2026-11-02; footprint entries from 2026-10-17, which the pinned CLI does not report";
const INSTRUCTIONS: i128 = 400_000_000;
const MEMORY: i128 = 41_943_040;
const READ_ENTRIES: i128 = 200;
const READ_BYTES: i128 = 200_000;
const WRITE_ENTRIES: i128 = 200;
const WRITE_BYTES: i128 = 132_096;
const FOOTPRINT: i128 = 400;
const EVENTS_BYTES: i128 = 16_384;

/// Every resource stays at or under this share of its network limit, in
/// hundredths of a percent.
const NETWORK_BAR: i128 = 7_000;

/// The test environment reads about half a point under the network on events,
/// so the events bar here is lower. At 69 percent, one transfer more than the
/// cap fails this suite instead of slipping through at 69.3 percent.
const ENV_EVENTS_BAR: i128 = 6_900;

/// Bytes left under the 70 percent events line at the all-tagged worst case,
/// as measured. Dropping below this is a re-measurement, not a judgement call:
/// any new field on any event in the execute path eats straight into it.
const RECORDED_HEADROOM_BYTES: i128 = 536;

const NOW: u64 = 1_800_000_000;
const DAY: u64 = 86_400;
const LEG_AMOUNT: i128 = 1_000_000;

/// The fields of the events the batch paths publish, without the `payout_id`
/// topic. Anything added here changes every number in this file.
const PAYOUT_REQUESTED: [&str; 8] = [
    "agent",
    "expires_at",
    "fee",
    "legs",
    "recipient_ref",
    "status",
    "task_ref",
    "total",
];
const PAYOUT_EXECUTED: [&str; 6] = ["day", "fee", "legs", "recipient_ref", "spent", "total"];

// The wasm under test.

fn soroban_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn newest_source(dir: &Path, skip: &Path, newest: &mut SystemTime) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.starts_with(skip) {
            continue;
        }
        if path.is_dir() {
            newest_source(&path, skip, newest);
        } else {
            let t = std::fs::metadata(&path).unwrap().modified().unwrap();
            if t > *newest {
                *newest = t;
            }
        }
    }
}

/// The release wasm `scripts/soroban-build.sh` writes. A build older than its
/// source is refused, so these numbers can never describe yesterday's code.
fn load_wasm(file: &str, contract: &str) -> StdVec<u8> {
    let root = soroban_dir();
    let path = root.join("target/wasm32v1-none/release").join(file);
    let built = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .unwrap_or_else(|e| {
            panic!(
                "{}: {e}; build the contracts first with scripts/soroban-build.sh",
                path.display()
            )
        });
    let dir = root.join("contracts").join(contract);
    let mut newest = SystemTime::UNIX_EPOCH;
    for src in [
        dir.join("src"),
        root.join("crates/mandate-config/src"),
        root.join("crates/mandate-errors/src"),
    ] {
        newest_source(&src, &dir.join("src/test"), &mut newest);
    }
    assert!(
        built >= newest,
        "{} is older than the contract source; rebuild it with scripts/soroban-build.sh",
        path.display()
    );
    std::fs::read(&path).unwrap()
}

fn mandate_wasm() -> &'static [u8] {
    static WASM: OnceLock<StdVec<u8>> = OnceLock::new();
    WASM.get_or_init(|| load_wasm("ujira_mandate.wasm", "mandate"))
}

fn factory_wasm() -> &'static [u8] {
    static WASM: OnceLock<StdVec<u8>> = OnceLock::new();
    WASM.get_or_init(|| load_wasm("ujira_factory.wasm", "factory"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// Shares of a limit, in hundredths of a percent, rounded up.

fn share(value: i128, limit: i128) -> i128 {
    (value * 10_000 + limit - 1) / limit
}

fn percent(share: i128) -> String {
    format!("{}.{:02}%", share / 100, share % 100)
}

fn events_line() -> i128 {
    EVENTS_BYTES * NETWORK_BAR / 10_000
}

// The world.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Dest {
    /// A classic account with a trustline, the usual Stellar wallet.
    Account,
    /// The same kind of account through a muxed `M` address: every transfer
    /// event gains `to_muxed_id` and every leg in the mandate's events widens,
    /// so this is the event size worst case.
    Tagged,
    /// Tagged and untagged legs in turn.
    Mixed,
}

struct World {
    env: Env,
    mandate: Address,
    agent: Address,
    asset: xdr::Asset,
    key: SigningKey,
    mode: u32,
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

/// A funded classic account holding an authorized trustline for the asset, so
/// a transfer to it touches the same entries it does on the network.
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

fn muxed(env: &Env, account: &Address, id: u64) -> MuxedAddress {
    let key = match ScAddress::from(account) {
        ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(k))) => k,
        other => panic!("not an account: {other:?}"),
    };
    let sc = ScVal::Address(ScAddress::MuxedAccount(xdr::MuxedEd25519Account {
        id,
        ed25519: key,
    }));
    MuxedAddress::try_from_val(env, &sc).unwrap()
}

fn bytes32(env: &Env, tag: u8, n: u32) -> BytesN<32> {
    let mut raw = [tag; 32];
    raw[28..].copy_from_slice(&n.to_be_bytes());
    BytesN::from_array(env, &raw)
}

fn world(mode: u32) -> World {
    let env = Env::new_with_config(soroban_sdk::testutils::EnvTestConfig {
        capture_snapshot_at_drop: false,
    });
    env.mock_all_auths();
    // Compared with the limits afterwards, so an over-limit batch prints its
    // numbers instead of stopping at a generic budget error.
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

    // A public test label. These runs only measure, and nothing holds funds.
    let seed: [u8; 32] = Sha256::digest(b"UJIRA/TEST-ONLY/batch-limits/attester").into();
    let key = SigningKey::from_bytes(&seed);
    let attester =
        (mode != MODE_ALLOWLIST).then(|| Bytes::from_array(&env, key.verifying_key().as_bytes()));

    let config = MandateConfig {
        principal: principal.clone(),
        asset: sac.address(),
        recipient_mode: mode,
        tenant_tag: bytes32(&env, 0x7e, 0),
        agents: Vec::from_array(&env, [agent.clone()]),
        approvers: Vec::new(&env),
        approvals_required: 0,
        guardians: Vec::new(&env),
        executors: Vec::new(&env),
        recovery: Vec::new(&env),
        recovery_threshold: 0,
        limits: MandateLimits {
            total_budget: i128::from(i64::MAX),
            per_payment_cap: i128::from(i64::MAX) / 4,
            per_recipient_daily_cap: i128::from(i64::MAX) / 4,
            // Above the payment cap, so every item lands APPROVED at once and
            // the same batch can be executed.
            approval_threshold: i128::from(i64::MAX) / 2,
            payout_ttl_seconds: DAY,
            expires_at: NOW + 30 * DAY,
        },
        attester,
        country_policy_hash: bytes32(&env, 0xc0, 0),
        max_fee_bps: 100,
    };

    let hash = env.deployer().upload_contract_wasm(mandate_wasm());
    let factory = env.register(
        factory_wasm(),
        (hash, Vec::from_array(&env, [sac.address()]), fee_address),
    );
    let mandate = UjiraFactoryClient::new(&env, &factory)
        .create(&config, &BytesN::from_array(&env, &[1; 32]));
    StellarAssetClient::new(&env, &sac.address()).mint(&mandate, &i128::from(i64::MAX));

    World {
        env,
        mandate,
        agent,
        asset,
        key,
        mode,
        next_account: 1,
    }
}

impl World {
    fn client(&self) -> UjiraMandateClient<'_> {
        UjiraMandateClient::new(&self.env, &self.mandate)
    }

    fn new_destination(&mut self, tagged: bool) -> MuxedAddress {
        let account = account_address(&self.env, self.next_account);
        self.next_account += 1;
        create_receiver(&self.env, &account, &self.asset);
        if tagged {
            // Every id is a full 8 bytes in XDR, whatever its value.
            muxed(&self.env, &account, u64::MAX - u64::from(self.next_account))
        } else {
            MuxedAddress::from(account)
        }
    }

    /// The mandate's own message, so the signature checks the attested path
    /// and not this suite's idea of the layout.
    fn attest(
        &self,
        recipient_ref: &BytesN<32>,
        destination: &MuxedAddress,
        valid_until: u64,
    ) -> Attestation {
        let policy_hash = bytes32(&self.env, 0xc0, 0);
        let message = self
            .env
            .as_contract(&self.mandate, || {
                ujira_mandate::attest::message(
                    &self.env,
                    recipient_ref,
                    &policy_hash,
                    0,
                    destination,
                    valid_until,
                )
            })
            .unwrap();
        let mut raw = std::vec![0u8; message.len() as usize];
        message.copy_into_slice(&mut raw);
        Attestation {
            valid_until,
            policy_epoch: 0,
            signature: Bytes::from_array(&self.env, &self.key.sign(&raw).to_bytes()),
        }
    }

    /// One request per plan entry: (legs, whether it carries a fee).
    fn build(&mut self, plan: &[(u32, bool)], dest: Dest) -> StdVec<PayoutRequest> {
        let mut reqs = StdVec::new();
        let mut allow = StdVec::new();
        let mut leg_number = 0u32;
        for (i, &(legs, fee)) in (0u32..).zip(plan.iter()) {
            let recipient_ref = bytes32(&self.env, 0x11, i);
            let mut leg_vec = Vec::new(&self.env);
            let mut atts = Vec::new(&self.env);
            for _ in 0..legs {
                let tagged = match dest {
                    Dest::Account => false,
                    Dest::Tagged => true,
                    Dest::Mixed => leg_number.is_multiple_of(2),
                };
                leg_number += 1;
                let destination = self.new_destination(tagged);
                if self.mode != MODE_ATTESTED {
                    allow.push(AllowEntry {
                        recipient_ref: recipient_ref.clone(),
                        destination: destination.clone(),
                    });
                }
                if self.mode != MODE_ALLOWLIST {
                    atts.push_back(self.attest(&recipient_ref, &destination, NOW + 3_600));
                }
                leg_vec.push_back(Leg {
                    destination,
                    amount: LEG_AMOUNT,
                });
            }
            let total = LEG_AMOUNT * i128::from(legs);
            reqs.push(PayoutRequest {
                payout_id: bytes32(&self.env, 0x22, i),
                recipient_ref,
                total,
                // The highest fee the 100 bps cap allows on this total.
                fee: if fee { total / 100 } else { 0 },
                legs: leg_vec,
                deadline: NOW + 600,
                task_ref: bytes32(&self.env, 0x33, i),
                attestations: atts,
            });
        }
        // The allowlist call takes a bounded batch too.
        for chunk in allow.chunks(MAX_BATCH as usize) {
            self.client()
                .allow_recipients(&Vec::from_slice(&self.env, chunk));
        }
        reqs
    }
}

// Measuring.

#[derive(Clone, Debug)]
struct Row {
    case: String,
    instructions: i128,
    memory: i128,
    read_entries: i128,
    footprint: i128,
    write_entries: i128,
    read_bytes: i128,
    write_bytes: i128,
    events: i128,
}

impl Row {
    fn shares(&self) -> [(&'static str, i128); 7] {
        [
            ("instructions", share(self.instructions, INSTRUCTIONS)),
            ("memory", share(self.memory, MEMORY)),
            ("read_entries", share(self.read_entries, READ_ENTRIES)),
            ("read_bytes", share(self.read_bytes, READ_BYTES)),
            ("write_entries", share(self.write_entries, WRITE_ENTRIES)),
            ("write_bytes", share(self.write_bytes, WRITE_BYTES)),
            ("footprint", share(self.footprint, FOOTPRINT)),
        ]
    }

    fn print(&self) {
        let parts: StdVec<String> = self
            .shares()
            .iter()
            .map(|(name, s)| format!("{name} {} ({})", self.raw(name), percent(*s)))
            .collect();
        println!(
            "{}: events {} of {} bytes ({}), {}",
            self.case,
            self.events,
            EVENTS_BYTES,
            percent(share(self.events, EVENTS_BYTES)),
            parts.join(", ")
        );
    }

    fn raw(&self, name: &str) -> i128 {
        match name {
            "instructions" => self.instructions,
            "memory" => self.memory,
            "read_entries" => self.read_entries,
            "read_bytes" => self.read_bytes,
            "write_entries" => self.write_entries,
            "write_bytes" => self.write_bytes,
            _ => self.footprint,
        }
    }

    fn assert_within_bars(&self) {
        self.print();
        for (name, s) in self.shares() {
            assert!(
                s <= NETWORK_BAR,
                "{}: {name} at {} of its limit, over the {} bar",
                self.case,
                percent(s),
                percent(NETWORK_BAR)
            );
        }
        let events = share(self.events, EVENTS_BYTES);
        assert!(
            events <= ENV_EVENTS_BAR,
            "{}: events at {} of the limit, over the {} bar for the test environment",
            self.case,
            percent(events),
            percent(ENV_EVENTS_BAR)
        );
    }
}

fn xdr_len<T: IntoVal<Env, Val> + Clone>(env: &Env, v: &T) -> u32 {
    use soroban_sdk::xdr::ToXdr;
    let val: Val = v.clone().into_val(env);
    val.to_xdr(env).len()
}

fn row(w: &World, case: String, return_bytes: u32) -> Row {
    let r = w.env.cost_estimate().resources();
    Row {
        case,
        instructions: i128::from(r.instructions),
        memory: i128::from(r.mem_bytes),
        read_entries: i128::from(r.disk_read_entries),
        footprint: i128::from(r.disk_read_entries) + i128::from(r.memory_read_entries),
        write_entries: i128::from(r.write_entries),
        read_bytes: i128::from(r.disk_read_bytes),
        write_bytes: i128::from(r.write_bytes),
        // The network counts the return value against the events limit.
        events: i128::from(r.contract_events_size_bytes) + i128::from(return_bytes),
    }
}

/// Fields of the mandate's events in the last call, by event name.
fn event_shapes(w: &World) -> StdVec<(String, StdVec<String>)> {
    let mut out = StdVec::new();
    for ev in w.env.events().all().filter_by_contract(&w.mandate).events() {
        let ContractEventBody::V0(body) = &ev.body;
        let Some(ScVal::Symbol(name)) = body.topics.first() else {
            continue;
        };
        out.push((name.0.to_utf8_string_lossy(), fields_of(&body.data)));
    }
    out
}

fn fields_of(data: &ScVal) -> StdVec<String> {
    let ScVal::Map(Some(map)) = data else {
        panic!("event data is not a map: {data:?}");
    };
    let mut keys: StdVec<String> = map
        .iter()
        .map(|e: &ScMapEntry| match &e.key {
            ScVal::Symbol(s) => s.0.to_utf8_string_lossy(),
            other => panic!("a key that is not a symbol: {other:?}"),
        })
        .collect();
    keys.sort();
    keys
}

/// Why an event no longer has the fields these limits were measured with.
fn shape_problem(name: &str, got: &[String], want: &[&str]) -> Option<String> {
    let want: StdVec<String> = want.iter().map(ToString::to_string).collect();
    (got != want.as_slice()).then(|| format!("{name} has fields {got:?}, measured with {want:?}"))
}

/// Why the all-tagged worst case no longer fits. A pure function, so the test
/// below can show one extra field really does fail it.
fn headroom_problem(events: i128) -> Option<String> {
    let left = events_line() - events;
    (left < RECORDED_HEADROOM_BYTES).then(|| {
        format!(
            "{left} bytes left under the 70 percent events line, recorded {RECORDED_HEADROOM_BYTES}: an event in the execute path grew, so measure again"
        )
    })
}

/// Requests the batch, checks every item landed APPROVED, executes it, checks
/// every item EXECUTED, and returns both measurements.
fn measure(mode: u32, plan: &[(u32, bool)], dest: Dest, execute: bool) -> (Row, Option<Row>) {
    assert!(plan.len() <= MAX_BATCH as usize);
    assert!(plan.iter().all(|p| p.0 <= MAX_LEGS));
    let legs: u32 = plan.iter().map(|p| p.0).sum();
    let fees = plan.iter().filter(|p| p.1).count();
    let label = format!(
        "{} items, {legs} legs, {fees} fees, {dest:?}, mode {mode}",
        plan.len()
    );
    let mut w = world(mode);
    let reqs = w.build(plan, dest);
    let batch = Vec::from_slice(&w.env, &reqs);
    let results = w.client().request_payouts(&w.agent, &batch);
    let requested = row(
        &w,
        format!("request_payouts {label}"),
        xdr_len(&w.env, &results),
    );
    for (name, fields) in event_shapes(&w) {
        if let Some(problem) = shape_problem(&name, &fields, &PAYOUT_REQUESTED) {
            panic!("{problem}");
        }
    }
    for r in results.iter() {
        assert_eq!(
            (r.code, r.status),
            (0, Some(STATUS_APPROVED)),
            "item refused"
        );
    }
    if !execute {
        return (requested, None);
    }

    let ids: Vec<BytesN<32>> = Vec::from_iter(&w.env, reqs.iter().map(|r| r.payout_id.clone()));
    w.client().execute_batch(&w.agent, &ids);
    // A unit return is ScVal::Void, 4 bytes of XDR.
    let executed = row(&w, format!("execute_batch {label}"), 4);
    for (name, fields) in event_shapes(&w) {
        if let Some(problem) = shape_problem(&name, &fields, &PAYOUT_EXECUTED) {
            panic!("{problem}");
        }
    }
    for r in &reqs {
        let status = w.client().payout(&r.payout_id).map(|p| match p {
            ujira_mandate::types::PayoutEntry::Record(rec) => rec.status,
            ujira_mandate::types::PayoutEntry::Tombstone(t) => t.status,
        });
        assert_eq!(status, Some(STATUS_EXECUTED));
    }
    (requested, Some(executed))
}

fn transfers(plan: &[(u32, bool)]) -> u32 {
    plan.iter().map(|&(legs, fee)| legs + u32::from(fee)).sum()
}

/// The plan that spends a transfer budget on the most event bytes: as many
/// items as allowed and every transfer a leg, because a leg also appears in
/// the mandate's own event and a fee transfer does not.
fn worst_plan(budget: u32) -> StdVec<(u32, bool)> {
    let items = MAX_BATCH.min(budget);
    (0..items)
        .map(|i| (budget / items + u32::from(i < budget % items), false))
        .collect()
}

fn execute_case(plan: &[(u32, bool)], dest: Dest) -> Row {
    assert!(transfers(plan) <= MAX_BATCH_TRANSFERS);
    let (_, executed) = measure(MODE_ALLOWLIST, plan, dest, true);
    executed.unwrap()
}

/// Names the wasm and the settings every other number here depends on.
#[test]
fn prints_the_production_wasm_and_the_settings() {
    println!(
        "production wasm: ujira_mandate.wasm sha256 {}, ujira_factory.wasm sha256 {}",
        hex(&Sha256::digest(mandate_wasm())),
        hex(&Sha256::digest(factory_wasm()))
    );
    println!("network settings: {SETTINGS_READ}");
    println!(
        "limits: instructions {INSTRUCTIONS}, memory {MEMORY}, read entries {READ_ENTRIES}, read bytes {READ_BYTES}, write entries {WRITE_ENTRIES}, write bytes {WRITE_BYTES}, footprint {FOOTPRINT}, events {EVENTS_BYTES}"
    );
    println!(
        "caps under test: MAX_BATCH {MAX_BATCH}, MAX_BATCH_TRANSFERS {MAX_BATCH_TRANSFERS}, MAX_LEGS {MAX_LEGS}"
    );
    assert_eq!(MAX_BATCH, 10, "a new cap needs a new measurement");
    assert_eq!(MAX_BATCH_TRANSFERS, 18, "a new cap needs a new measurement");
}

#[test]
fn execute_worst_cases_stay_under_the_bars() {
    let two_legs: StdVec<(u32, bool)> = (0..9).map(|_| (2, false)).collect();
    let mut with_fees: StdVec<(u32, bool)> = (0..3).map(|_| (4, true)).collect();
    with_fees.push((2, true));
    let single: StdVec<(u32, bool)> = (0..10).map(|_| (1, false)).collect();
    assert_eq!(transfers(&two_legs), 18);
    assert_eq!(transfers(&with_fees), 18);
    for plan in [&two_legs, &with_fees, &single] {
        for dest in [Dest::Account, Dest::Tagged] {
            execute_case(plan, dest).assert_within_bars();
        }
    }
}

#[test]
fn request_worst_cases_stay_under_the_bars() {
    let plan: StdVec<(u32, bool)> = (0..10).map(|_| (4, true)).collect();
    for mode in [MODE_BOTH, MODE_ATTESTED] {
        for dest in [Dest::Account, Dest::Tagged] {
            // 40 legs is over the transfer cap, so these are never executed.
            let (requested, _) = measure(mode, &plan, dest, false);
            requested.assert_within_bars();
        }
    }
}

#[test]
fn tagged_batches_at_the_transfer_cap_stay_under_the_bars() {
    let plan = worst_plan(MAX_BATCH_TRANSFERS);
    assert_eq!(transfers(&plan), MAX_BATCH_TRANSFERS);
    execute_case(&plan, Dest::Tagged).assert_within_bars();
    execute_case(&plan, Dest::Mixed).assert_within_bars();
}

/// The margin the transfer cap rests on. It is thin, so it is held to its
/// recorded size rather than only to the line, and one transfer past the cap
/// must already be over the bar for the test environment.
#[test]
fn headroom_under_the_events_line_is_as_recorded() {
    let at_cap = execute_case(&worst_plan(MAX_BATCH_TRANSFERS), Dest::Tagged);
    let below = execute_case(&worst_plan(MAX_BATCH_TRANSFERS - 1), Dest::Tagged);
    let left = events_line() - at_cap.events;
    let per_transfer = at_cap.events - below.events;
    println!(
        "all-tagged worst case: {} bytes, {left} bytes left under the 70 percent line of {}, recorded {RECORDED_HEADROOM_BYTES}, {per_transfer} bytes per tagged transfer",
        at_cap.events,
        events_line(),
    );
    if let Some(problem) = headroom_problem(at_cap.events) {
        panic!("{problem}");
    }
    let one_more = at_cap.events + per_transfer;
    assert!(
        share(one_more, EVENTS_BYTES) > ENV_EVENTS_BAR,
        "{one_more} bytes at one transfer past the cap is {}, so the cap could be raised unnoticed",
        percent(share(one_more, EVENTS_BYTES))
    );
}

/// A heavier event must fail the suite. One more field costs at least its
/// name and a small value in each of the batch's events.
#[test]
fn one_extra_event_field_fails_the_suite() {
    let at_cap = execute_case(&worst_plan(MAX_BATCH_TRANSFERS), Dest::Tagged);
    assert_eq!(headroom_problem(at_cap.events), None);
    // A symbol key of one character and a u32 value, in XDR.
    let field = i128::from(xdr_field_bytes());
    let heavier = at_cap.events + field * i128::from(MAX_BATCH);
    assert!(headroom_problem(heavier).is_some());

    let mut got: StdVec<String> = PAYOUT_EXECUTED.iter().map(ToString::to_string).collect();
    assert_eq!(
        shape_problem("payout_executed", &got, &PAYOUT_EXECUTED),
        None
    );
    got.push(String::from("x"));
    assert!(shape_problem("payout_executed", &got, &PAYOUT_EXECUTED).is_some());
}

fn xdr_field_bytes() -> u32 {
    use xdr::{Limits, ScSymbol, WriteXdr};
    let entry = ScMapEntry {
        key: ScVal::Symbol(ScSymbol("x".try_into().unwrap())),
        val: ScVal::U32(0),
    };
    entry
        .to_xdr(Limits::none())
        .unwrap()
        .len()
        .try_into()
        .unwrap()
}
