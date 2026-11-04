// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The Soroban driver: runs one vector against the compiled factory and
//! mandate wasm in the Soroban test host and records its trace.
//!
//! Only compiled wasm is registered for the factory and the mandate, never a
//! native test contract, because the host reports some errors differently
//! through a native contract than a network does. Calls are authorized with
//! real ed25519 signatures by the signing actor over the root invocation, so
//! a call signed by the wrong actor fails host authorization exactly as it
//! would on a network.
//!
//! `tests/README.md` describes the contract interface this driver expects.

mod ledger;
pub mod scval;
pub mod smart;

use crate::attest::{self, AttestInput, Attestation, Tamper};
use crate::spec::{mode_code, Spec};
use crate::trace::status;
use crate::values::{
    actor_key, actor_seed, bytes32, hex, is_label, is_name, label_bytes, label_key, parse_amount,
    parse_hex, parse_u32, parse_u64, sha256, str_of,
};
use crate::vectors::Vector;
use crate::Result;
use ed25519_dalek::{Signer, SigningKey};
use scval::{account, address_xdr, bytes, i128_val, record, vec as sc_vec, Names};
use serde_json::{json, Map, Value};
use soroban_sdk::testutils::{
    EnvTestConfig, Events as _, IssuerFlags, Ledger as _, MockAuthContract,
};
use soroban_sdk::xdr::{
    self, ContractDataDurability, ContractExecutable, Hash, HashIdPreimage,
    HashIdPreimageSorobanAuthorization, InvokeContractArgs, LedgerEntryData, LedgerKey,
    LedgerKeyContractData, Limits, ScAddress, ScError, ScMap, ScMapEntry, ScSymbol, ScVal,
    SorobanAddressCredentials, SorobanAuthorizationEntry, SorobanAuthorizedFunction,
    SorobanAuthorizedInvocation, SorobanCredentials, VecM, WriteXdr,
};
use soroban_sdk::{Address, Env, IntoVal, Symbol, TryFromVal, Val};
use std::collections::{BTreeMap, BTreeSet};
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::rc::Rc;

/// The network the mandate runs on in the test host, so attestations bind
/// the same network ID they would on testnet.
pub const NETWORK_PASSPHRASE: &str = "Test SDF Network ; September 2015";

/// Time moves the ledger sequence on by one ledger per five seconds, as the
/// spec's TTL arithmetic assumes.
pub const LEDGER_SECONDS: u64 = 5;

const START_SEQUENCE: u32 = 1_000;
// Testnet's archival settings, as the batch-limits spike read them, so
// records live and archive the way they do there.
const MIN_TEMP_TTL: u32 = 720;
const MIN_PERSISTENT_TTL: u32 = 120_960;
const MAX_ENTRY_TTL: u32 = 3_110_400;

/// The runner's own `other_token` kinds, built from
/// `contracts/test-token` and checked against its source in CI.
pub const TEST_TOKEN_WASM: &[u8] = include_bytes!("../../fixtures/vector_runner_test_token.wasm");

/// Functions whose first argument is the caller, which the call authorizes.
pub const CALLER_ARG: [&str; 12] = [
    "request_payout",
    "request_payouts",
    "approve",
    "reject",
    "cancel",
    "cancel_batch",
    "execute",
    "execute_batch",
    "pause",
    "sweep",
    "recovery_revoke",
    "recovery_rotate_agent",
];

/// Views need no authorization and change nothing.
pub const VIEWS: [&str; 6] = [
    "version",
    "config",
    "state",
    "payout",
    "daily_used",
    "is_allowed",
];

/// The `state()` view, all of it.
const STATE_FIELDS: [&str; 9] = [
    "spent",
    "pending",
    "committed",
    "balance",
    "free",
    "paused",
    "revoked",
    "expired",
    "policy_epoch",
];

/// The mandate fields a vector's state can name that `config()` reports.
const CONFIG_FIELDS: [&str; 16] = [
    "recovery_epoch",
    "agent_epoch",
    "approver_epoch",
    "approvals_required",
    "recovery_threshold",
    "max_fee_bps",
    "principal",
    "proposed_principal",
    "agents",
    "approvers",
    "guardians",
    "executors",
    "recovery",
    "limits",
    "attester",
    "country_policy_hash",
];

/// The Stellar Asset Contract's code for an account with no trustline.
const SAC_TRUSTLINE_MISSING: u32 = 13;

/// The compiled factory and mandate a run registers, and the passkey wallet
/// factory when it is built beside the mandate, for smart account actors.
pub struct Contracts {
    pub factory: Vec<u8>,
    pub mandate: Vec<u8>,
    pub wallet_factory: Option<Vec<u8>>,
}

impl Contracts {
    pub fn load(factory: &Path, mandate: &Path) -> Result<Contracts> {
        let read =
            |p: &Path| std::fs::read(p).map_err(|e| format!("cannot read {}: {e}", p.display()));
        let wallet_factory = mandate
            .parent()
            .and_then(|dir| std::fs::read(dir.join("ujira_wallet_factory.wasm")).ok());
        Ok(Contracts {
            factory: read(factory)?,
            mandate: read(mandate)?,
            wallet_factory,
        })
    }

    pub fn describe(&self) -> Value {
        json!({
            "factory_wasm_sha256": hex(&sha256(&self.factory)),
            "mandate_wasm_sha256": hex(&sha256(&self.mandate)),
        })
    }
}

/// What one invocation came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ok(ScVal),
    /// A contract error code raised by the contract that was called.
    Contract(u32),
    /// A contract error code raised by another contract further down, such
    /// as a token, that escaped the called contract unchanged. The Stellar
    /// Asset Contract's codes overlap the mandate's (its 10, 11 and 12 are
    /// UNAUTHORIZED, ROLE_CONFLICT and ROLE_SET_FULL), so the code alone
    /// never says whose error it is.
    Escaped {
        origin: ScAddress,
        code: u32,
    },
    /// A trap in a host function, such as a failed ed25519 verification.
    HostAbort(String),
    /// Host authorization refused the call.
    AuthFailure(String),
    /// Any other host error, written `Type.Code`.
    HostError(String),
    /// The test host itself panicked, for example over a resource limit.
    Panic(String),
}

fn error_text(e: &ScError) -> String {
    let code = match e {
        ScError::Contract(c) => return format!("Contract.{c}"),
        ScError::WasmVm(c)
        | ScError::Context(c)
        | ScError::Storage(c)
        | ScError::Object(c)
        | ScError::Crypto(c)
        | ScError::Events(c)
        | ScError::Budget(c)
        | ScError::Value(c)
        | ScError::Auth(c) => c,
    };
    format!("{}.{}", e.discriminant().name(), code.name())
}

pub fn classify(err: soroban_sdk::Error) -> Outcome {
    match ScError::try_from(err) {
        Ok(ScError::Contract(code)) => Outcome::Contract(code),
        Ok(e @ ScError::Auth(_)) => Outcome::AuthFailure(error_text(&e)),
        Ok(e @ ScError::Crypto(_)) => Outcome::HostAbort(error_text(&e)),
        Ok(e) => Outcome::HostError(error_text(&e)),
        Err(_) => Outcome::HostError(format!("{err:?}")),
    }
}

fn panic_text(p: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = p.downcast_ref::<String>() {
        return s.clone();
    }
    String::from("the test host panicked")
}

pub fn to_address(env: &Env, a: &ScAddress) -> Result<Address> {
    Address::try_from_val(env, &ScVal::Address(a.clone()))
        .map_err(|e| format!("cannot use the address {a:?}: {e:?}"))
}

/// Per-transaction limits of the network, read from `stellar network
/// settings`; testnet and mainnet agree on every one. A call over any of
/// them would never land, so the runner reports it as a host error.
pub struct NetworkLimits;
impl NetworkLimits {
    pub const INSTRUCTIONS: i64 = 400_000_000;
    pub const MEMORY: i64 = 41_943_040;
    pub const DISK_READ_ENTRIES: u32 = 200;
    pub const DISK_READ_BYTES: u32 = 200_000;
    pub const WRITE_ENTRIES: u32 = 200;
    pub const WRITE_BYTES: u32 = 132_096;
    pub const FOOTPRINT_ENTRIES: u32 = 400;
    pub const EVENTS_BYTES: u32 = 16_384;
}

/// What one invocation used, as the network counts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallResources {
    pub instructions: i64,
    pub memory_bytes: i64,
    pub read_entries: u32,
    pub write_entries: u32,
    pub read_bytes: u32,
    pub write_bytes: u32,
}

/// What the last invocation used, if the host recorded it.
pub fn last_invocation(env: &Env) -> Option<CallResources> {
    let r = env.host().get_last_invocation_resources()?;
    Some(CallResources {
        instructions: r.instructions,
        memory_bytes: r.mem_bytes,
        read_entries: r.disk_read_entries + r.memory_read_entries,
        write_entries: r.write_entries,
        read_bytes: r.disk_read_bytes,
        write_bytes: r.write_bytes,
    })
}

/// Lifts the test host's own invocation limits. A signed call records an
/// authorization snapshot that a network never builds, and with a large
/// batch the snapshot alone runs the host out of budget after the contract
/// has finished. The network's limits are checked by hand instead, in
/// `over_network_limit`, against what the call itself used.
pub fn lift_host_limits(env: &Env) {
    env.cost_estimate().disable_resource_limits();
}

/// The first network limit the last invocation went over, if any.
pub fn over_network_limit(env: &Env) -> Option<String> {
    let r = env.host().get_last_invocation_resources()?;
    let checks = [
        ("instructions", r.instructions, NetworkLimits::INSTRUCTIONS),
        ("memory", r.mem_bytes, NetworkLimits::MEMORY),
        (
            "disk_read_entries",
            r.disk_read_entries.into(),
            NetworkLimits::DISK_READ_ENTRIES.into(),
        ),
        (
            "disk_read_bytes",
            r.disk_read_bytes.into(),
            NetworkLimits::DISK_READ_BYTES.into(),
        ),
        (
            "write_entries",
            r.write_entries.into(),
            NetworkLimits::WRITE_ENTRIES.into(),
        ),
        (
            "write_bytes",
            r.write_bytes.into(),
            NetworkLimits::WRITE_BYTES.into(),
        ),
        (
            "footprint_entries",
            i64::from(r.disk_read_entries) + i64::from(r.memory_read_entries),
            NetworkLimits::FOOTPRINT_ENTRIES.into(),
        ),
        (
            "contract_events_size_bytes",
            r.contract_events_size_bytes.into(),
            NetworkLimits::EVENTS_BYTES.into(),
        ),
    ];
    checks
        .iter()
        .find(|(_, used, limit)| used > limit)
        .map(|(name, used, limit)| format!("Budget.ExceededLimit: {name} {used} of {limit}"))
}

/// Calls `fn_name` on `target` with whatever authorization the env holds,
/// and says how it ended.
pub fn call(env: &Env, target: &ScAddress, fn_name: &str, args: &[ScVal]) -> Result<Outcome> {
    let outcome = call_unchecked(env, target, fn_name, args)?;
    if matches!(outcome, Outcome::Panic(_)) {
        return Ok(outcome);
    }
    // A call that needed more than a transaction may use would not land,
    // whatever the contract answered.
    Ok(match over_network_limit(env) {
        Some(text) => Outcome::HostError(text),
        None => outcome,
    })
}

fn call_unchecked(env: &Env, target: &ScAddress, fn_name: &str, args: &[ScVal]) -> Result<Outcome> {
    let addr = to_address(env, target)?;
    let mut vals = soroban_sdk::Vec::<Val>::new(env);
    for a in args {
        vals.push_back(Val::try_from_val(env, a).map_err(|e| format!("cannot pass {a:?}: {e:?}"))?);
    }
    let func = Symbol::new(env, fn_name);
    let got = std::panic::catch_unwind(AssertUnwindSafe(|| {
        env.try_invoke_contract::<Val, soroban_sdk::Error>(&addr, &func, vals)
    }));
    Ok(match got {
        Err(p) => Outcome::Panic(panic_text(p)),
        Ok(Ok(Ok(val))) => Outcome::Ok(
            ScVal::try_from_val(env, &val).map_err(|e| format!("cannot read the result: {e:?}"))?,
        ),
        Ok(Ok(Err(e))) => return Err(format!("cannot read the result: {e:?}")),
        Ok(Err(Ok(err))) => {
            let errors = diagnostic_errors(env);
            match classify(err) {
                Outcome::Contract(code) => {
                    let wanted = ScError::Contract(code);
                    match errors.iter().find(|(e, _)| *e == wanted) {
                        Some((_, Some(origin))) if origin != target => Outcome::Escaped {
                            origin: origin.clone(),
                            code,
                        },
                        _ => Outcome::Contract(code),
                    }
                }
                // A wasm contract that fails on a host error, an auth or a
                // crypto failure among them, reaches its caller as a generic
                // context error; the cause is the first host error recorded.
                other => match errors
                    .iter()
                    .find(|(e, _)| !matches!(e, ScError::Contract(_)))
                {
                    Some((root, _)) => match classify(soroban_sdk::Error::from(root.clone())) {
                        Outcome::Contract(_) => other,
                        refined => refined,
                    },
                    None => other,
                },
            }
        }
        Ok(Err(Err(e))) => Outcome::HostError(format!("{e:?}")),
    })
}

/// Every error the host recorded in the last invocation, in order, with the
/// contract whose frame it happened in, from the diagnostic events.
fn diagnostic_errors(env: &Env) -> Vec<(ScError, Option<ScAddress>)> {
    let Ok(events) = env.host().get_diagnostic_events() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for he in events.0.iter() {
        let ev = &he.event;
        if ev.type_ != xdr::ContractEventType::Diagnostic {
            continue;
        }
        let xdr::ContractEventBody::V0(body) = &ev.body;
        let is_error =
            matches!(body.topics.first(), Some(ScVal::Symbol(s)) if s.0.as_slice() == b"error");
        if let (true, Some(ScVal::Error(e))) = (is_error, body.topics.get(1)) {
            let origin = ev.contract_id.clone().map(ScAddress::Contract);
            out.push((e.clone(), origin));
        }
    }
    out
}

/// The wasm hash a contract instance runs, or None for anything that is not
/// a wasm contract.
pub fn executable_hash(env: &Env, contract: &ScAddress) -> Result<Option<[u8; 32]>> {
    let key = LedgerKey::ContractData(LedgerKeyContractData {
        contract: contract.clone(),
        key: ScVal::LedgerKeyContractInstance,
        durability: ContractDataDurability::Persistent,
    });
    let found = env
        .host()
        .get_ledger_entry(&Rc::new(key))
        .map_err(|e| format!("cannot read a contract instance: {e:?}"))?;
    Ok(match found {
        Some((entry, _)) => match &entry.data {
            LedgerEntryData::ContractData(d) => match &d.val {
                ScVal::ContractInstance(inst) => match &inst.executable {
                    ContractExecutable::Wasm(Hash(h)) => Some(*h),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        },
        None => None,
    })
}

#[derive(Clone)]
struct Actor {
    address: ScAddress,
    /// The signing key of an account; a contract actor signs through the
    /// SDK's mock account contract instead.
    key: Option<SigningKey>,
    /// A passkey wallet's keys, for a smart account actor.
    smart: Option<smart::SmartAccount>,
    /// The test account whose `__check_auth` calls back into its caller.
    calls_back: bool,
}

/// Who the actor named `principal` is in a run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PrincipalMode {
    /// As the vector says: an account unless it declares otherwise.
    #[default]
    AsDeclared,
    /// A passkey wallet with three passkeys and a threshold of two, wherever
    /// the vector does not declare the principal itself.
    SmartAccount,
}

/// The passkeys and threshold of the principal under `PrincipalMode::SmartAccount`.
const SMART_PRINCIPAL: (u32, u32) = (3, 2);

/// The wallet contracts, named so their events are told apart from the
/// mandate's.
const WALLET_CONTRACTS: [&str; 3] = ["wallet_factory", "wallet_verifier", "wallet_policy"];

/// Why a vector cannot run with a smart account principal, if it cannot: a
/// contract holds no trustline and has no ed25519 key, and a vector that
/// declares its principal is left as it is.
pub fn smart_principal_unsupported(v: &Vector) -> Option<String> {
    if v.setup()["actors"].get("principal").is_some() {
        return Some(String::from("the vector declares its own principal"));
    }
    for step in v.steps() {
        if step["step"] == "set_receiving" && step["account"] == "principal" {
            return Some(String::from("it changes the principal's trustline"));
        }
    }
    if mentions_account_key(&v.data, "principal") {
        return Some(String::from("it uses the principal's ed25519 key"));
    }
    None
}

fn mentions_account_key(v: &Value, name: &str) -> bool {
    match v {
        Value::Object(m) => m
            .iter()
            .any(|(k, val)| (k == "account_key" && val == name) || mentions_account_key(val, name)),
        Value::Array(a) => a.iter().any(|x| mentions_account_key(x, name)),
        _ => false,
    }
}

#[derive(Clone)]
struct Token {
    address: ScAddress,
    /// The classic asset behind a Stellar Asset Contract; None for the
    /// runner's own test token.
    asset: Option<xdr::Asset>,
}

enum Auth {
    None,
    Mock,
    Signer(String),
    /// A smart account signing with this many of its passkeys, instead of
    /// its threshold.
    SignerWith(String, u32),
}

const TEXT_KEYS: [&str; 3] = ["id", "rules", "description"];
const ACTOR_KEYS: [&str; 19] = [
    "principal",
    "caller",
    "signed_by",
    "from",
    "to",
    "account",
    "address",
    "proposed",
    "old",
    "new",
    "by",
    "current",
    "previous",
    "approver",
    "agent",
    "account_key",
    "proposed_principal",
    "mandate",
    "fee_address",
];
const ACTOR_LISTS: [&str; 6] = [
    "agents",
    "approvers",
    "guardians",
    "executors",
    "recovery",
    "approvals",
];

/// Every actor name the vector mentions, wherever it can stand.
fn collect_actors(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::Object(o) => {
            for (k, val) in o {
                if TEXT_KEYS.contains(&k.as_str()) {
                    continue;
                }
                if k == "balances" {
                    for holders in val.as_object().into_iter().flat_map(|t| t.values()) {
                        if let Some(h) = holders.as_object() {
                            out.extend(h.keys().cloned());
                        }
                    }
                    continue;
                }
                if ACTOR_KEYS.contains(&k.as_str()) {
                    if let Some(s) = val.as_str().filter(|s| is_name(s)) {
                        out.insert(s.to_string());
                    }
                }
                if ACTOR_LISTS.contains(&k.as_str()) {
                    for s in val
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                    {
                        out.insert(s.to_string());
                    }
                }
                collect_actors(val, out);
            }
        }
        Value::Array(items) => {
            for x in items {
                collect_actors(x, out);
            }
        }
        _ => {}
    }
}

/// Every label the vector writes, and the 32-byte values the run must read
/// back after each step: payout IDs and recipient refs.
#[derive(Default)]
struct Mentions {
    labels: BTreeSet<String>,
    payout_ids: BTreeSet<[u8; 32]>,
    refs: BTreeSet<[u8; 32]>,
}

fn collect_mentions(v: &Value, out: &mut Mentions) {
    match v {
        Value::Object(o) => {
            for (k, val) in o {
                if is_label(k) {
                    out.labels.insert(k.clone());
                }
                let as32 = |s: &str| bytes32(s).ok();
                match (k.as_str(), val) {
                    ("payout_id", Value::String(s)) => out.payout_ids.extend(as32(s)),
                    ("recipient_ref", Value::String(s)) => out.refs.extend(as32(s)),
                    ("ids", Value::Array(l)) => {
                        out.payout_ids
                            .extend(l.iter().filter_map(Value::as_str).filter_map(as32));
                    }
                    ("payouts", Value::Object(m)) => {
                        out.payout_ids.extend(m.keys().filter_map(|s| as32(s)));
                    }
                    ("daily", Value::Object(m)) => {
                        out.refs.extend(m.keys().filter_map(|s| as32(s)));
                    }
                    _ => {}
                }
                collect_mentions(val, out);
            }
        }
        Value::Array(items) => {
            for x in items {
                collect_mentions(x, out);
            }
        }
        Value::String(s) if is_label(s) => {
            out.labels.insert(s.clone());
        }
        _ => {}
    }
}

/// One vector's world: the env, its actors and tokens, the factory, and the
/// mandate once it exists.
pub struct World<'a> {
    /// What the last call step used. Read here because every step goes on to
    /// read the state through views, which replace the host's own record.
    last_call: Option<CallResources>,
    env: Env,
    spec: &'a Spec,
    actors: BTreeMap<String, Actor>,
    reverse: BTreeMap<Vec<u8>, String>,
    labels: BTreeMap<[u8; 32], String>,
    key_labels: BTreeMap<[u8; 32], String>,
    account_keys: BTreeMap<[u8; 32], String>,
    asset: Token,
    other: Token,
    factory: ScAddress,
    mandate: Option<ScAddress>,
    attestations: BTreeMap<String, Attestation>,
    payout_ids: BTreeSet<[u8; 32]>,
    refs: BTreeSet<[u8; 32]>,
    nonce: i64,
}

/// What `setup.actors` says an actor is, beyond an account.
enum Declared {
    Contract,
    Smart(u32, u32),
    CallsBack,
}

fn declared_actors(setup: &Value) -> Result<BTreeMap<String, Declared>> {
    let mut out = BTreeMap::new();
    for (name, spec) in setup["actors"].as_object().into_iter().flatten() {
        let kind = match spec["kind"].as_str().unwrap_or("") {
            "contract" => Declared::Contract,
            "calls_back_account" => Declared::CallsBack,
            "smart_account" => {
                let n = spec["signers"].as_u64().unwrap_or(1);
                let t = spec["threshold"].as_u64().unwrap_or(n);
                let n = u32::try_from(n).map_err(|_| format!("{name}: too many signers"))?;
                let t = u32::try_from(t).map_err(|_| format!("{name}: threshold too large"))?;
                Declared::Smart(n, t)
            }
            _ => continue,
        };
        out.insert(name.clone(), kind);
    }
    Ok(out)
}

impl Names for World<'_> {
    fn name_of(&self, a: &ScAddress) -> Option<String> {
        self.reverse.get(&address_xdr(a).ok()?).cloned()
    }

    fn bytes32(&self, b: &[u8; 32]) -> String {
        self.labels.get(b).cloned().unwrap_or_else(|| hex(b))
    }

    fn attester(&self, b: &[u8]) -> Value {
        if let Ok(k) = <[u8; 32]>::try_from(b) {
            if let Some(l) = self.key_labels.get(&k) {
                return Value::String(l.clone());
            }
            if let Some(a) = self.account_keys.get(&k) {
                return json!({ "account_key": a });
            }
        }
        Value::String(hex(b))
    }
}

impl<'a> World<'a> {
    /// Builds the env for `v`: ledger, tokens, actors and factory. Nothing
    /// of the vector's setup has run yet.
    pub fn new(spec: &'a Spec, contracts: &Contracts, v: &Vector) -> Result<World<'a>> {
        World::with_principal(spec, contracts, v, PrincipalMode::AsDeclared)
    }

    /// As `new`, with the principal chosen by `mode`.
    pub fn with_principal(
        spec: &'a Spec,
        contracts: &Contracts,
        v: &Vector,
        mode: PrincipalMode,
    ) -> Result<World<'a>> {
        let setup = v.setup();
        let now = parse_u64(&setup["now"], "setup.now")?;
        let env = Env::new_with_config(EnvTestConfig {
            capture_snapshot_at_drop: false,
        });
        lift_host_limits(&env);
        let network_id = sha256(NETWORK_PASSPHRASE.as_bytes());
        env.ledger().with_mut(|li| {
            li.timestamp = now;
            li.sequence_number = START_SEQUENCE;
            li.network_id = network_id;
            li.min_temp_entry_ttl = MIN_TEMP_TTL;
            li.min_persistent_entry_ttl = MIN_PERSISTENT_TTL;
            li.max_entry_ttl = MAX_ENTRY_TTL;
        });

        let mut names = BTreeSet::new();
        collect_actors(&v.data, &mut names);
        names.insert(String::from("fee_address"));
        if names.contains("zero") {
            return Err(String::from(
                "the zero address cannot be expressed on Stellar",
            ));
        }
        let mut declared = declared_actors(setup)?;
        if mode == PrincipalMode::SmartAccount
            && names.contains("principal")
            && !declared.contains_key("principal")
        {
            declared.insert(
                String::from("principal"),
                Declared::Smart(SMART_PRINCIPAL.0, SMART_PRINCIPAL.1),
            );
        }
        let kit = if declared.values().any(|d| matches!(d, Declared::Smart(..))) {
            let wasm = contracts.wallet_factory.as_ref().ok_or_else(|| {
                String::from(
                    "a smart account needs the wallet factory wasm; build it with scripts/soroban-build.sh",
                )
            })?;
            Some(smart::Kit::deploy(&env, wasm)?)
        } else {
            None
        };

        let (asset, other) =
            std::panic::catch_unwind(AssertUnwindSafe(|| make_tokens(&env, setup)))
                .map_err(|p| format!("the test tokens could not be set up: {}", panic_text(p)))??;

        let mut actors = BTreeMap::new();
        let mut account_keys = BTreeMap::new();
        for name in names
            .iter()
            .filter(|n| !["mandate", "factory", "asset", "other_token"].contains(&n.as_str()))
        {
            // Addresses nobody controls: no account entry, no contract and
            // no key, so they can be named as destinations and never sign.
            let unsigned = |address| Actor {
                address,
                key: None,
                smart: None,
                calls_back: false,
            };
            let actor = if name == "zero_account" {
                unsigned(account([0; 32]))
            } else if name == "zero_contract" {
                unsigned(scval::contract([0; 32]))
            } else if let (Some(Declared::Smart(n, t)), Some(kit)) = (declared.get(name), &kit) {
                let (address, wallet) = smart::create(&env, kit, name, *n, *t)?;
                Actor {
                    address: ScAddress::from(&address),
                    key: None,
                    smart: Some(wallet),
                    calls_back: false,
                }
            } else if let Some(Declared::CallsBack) = declared.get(name) {
                let address = scval::contract(actor_seed(name));
                env.register_at(&to_address(&env, &address)?, smart::CallsBackAccount, ());
                Actor {
                    calls_back: true,
                    ..unsigned(address)
                }
            } else if let Some(Declared::Contract) = declared.get(name) {
                let address = scval::contract(actor_seed(name));
                let a = to_address(&env, &address)?;
                // The SDK's mock account contract accepts any signature, so a
                // contract actor can authorize a call.
                env.register_at(&a, MockAuthContract, ());
                unsigned(address)
            } else {
                let key = actor_key(name);
                let pk = key.verifying_key().to_bytes();
                ledger::create_account(&env, pk)?;
                if let Some(a) = &asset.asset {
                    ledger::set_trustline(&env, pk, a, ledger::AUTHORIZED | ledger::CLAWBACK)?;
                }
                if let Some(a) = &other.asset {
                    ledger::set_trustline(&env, pk, a, ledger::AUTHORIZED)?;
                }
                account_keys.insert(pk, name.clone());
                Actor {
                    address: account(pk),
                    key: Some(key),
                    smart: None,
                    calls_back: false,
                }
            };
            actors.insert(name.clone(), actor);
        }

        let fee_address = actors
            .get("fee_address")
            .map(|a| a.address.clone())
            .ok_or_else(|| String::from("no fee address"))?;
        let factory = std::panic::catch_unwind(AssertUnwindSafe(|| {
            register_factory(&env, contracts, &asset.address, &fee_address)
        }))
        .map_err(|p| format!("the factory could not be registered: {}", panic_text(p)))??;

        let mut reverse = BTreeMap::new();
        for (name, a) in &actors {
            reverse.insert(address_xdr(&a.address)?, name.clone());
        }
        reverse.insert(address_xdr(&asset.address)?, String::from("asset"));
        reverse.insert(address_xdr(&other.address)?, String::from("other_token"));
        reverse.insert(address_xdr(&factory)?, String::from("factory"));
        if let Some(kit) = &kit {
            for (name, a) in WALLET_CONTRACTS
                .iter()
                .zip([&kit.factory, &kit.verifier, &kit.policy])
            {
                reverse.insert(address_xdr(&ScAddress::from(a))?, (*name).to_string());
            }
        }

        let mut mentions = Mentions::default();
        collect_mentions(&v.data, &mut mentions);
        let mut labels = BTreeMap::new();
        let mut key_labels = BTreeMap::new();
        for l in &mentions.labels {
            labels.insert(label_bytes(l)?, l.clone());
            key_labels.insert(label_key(l)?.verifying_key().to_bytes(), l.clone());
        }

        Ok(World {
            last_call: None,
            env,
            spec,
            actors,
            reverse,
            labels,
            key_labels,
            account_keys,
            asset,
            other,
            factory,
            mandate: None,
            attestations: BTreeMap::new(),
            payout_ids: mentions.payout_ids,
            refs: mentions.refs,
            nonce: 0,
        })
    }

    pub fn env(&self) -> &Env {
        &self.env
    }

    pub fn factory(&self) -> &ScAddress {
        &self.factory
    }

    pub fn mandate(&self) -> Option<&ScAddress> {
        self.mandate.as_ref()
    }

    /// The address a vector name stands for: an actor, the mandate, the
    /// factory or a token.
    pub fn address_of(&self, name: &str) -> Result<ScAddress> {
        match name {
            "mandate" => self
                .mandate
                .clone()
                .ok_or_else(|| String::from("the mandate does not exist yet")),
            "factory" => Ok(self.factory.clone()),
            "asset" => Ok(self.asset.address.clone()),
            "other_token" => Ok(self.other.address.clone()),
            _ => self
                .actors
                .get(name)
                .map(|a| a.address.clone())
                .ok_or_else(|| format!("{name} is not an actor of this vector")),
        }
    }

    fn token(&self, name: &str) -> Result<&Token> {
        match name {
            "asset" => Ok(&self.asset),
            "other_token" => Ok(&self.other),
            _ => Err(format!("{name} is not a token")),
        }
    }

    fn set_mandate(&mut self, a: ScAddress) -> Result<()> {
        self.reverse
            .insert(address_xdr(&a)?, String::from("mandate"));
        self.mandate = Some(a);
        Ok(())
    }

    fn decode(&self, v: &ScVal, key: &str) -> Result<Value> {
        let spec = self.spec;
        scval::decode(v, key, self, &|c| spec.error_name(c).map(String::from))
    }

    /// The authorization entry `signer` signs for one root invocation. A
    /// smart account signs with `count` passkeys, or with its threshold.
    fn auth_entry(
        &mut self,
        signer: &str,
        target: &ScAddress,
        fn_name: &str,
        args: &[ScVal],
        count: Option<u32>,
    ) -> Result<SorobanAuthorizationEntry> {
        let actor =
            self.actors.get(signer).cloned().ok_or_else(|| {
                format!("{signer} cannot sign: it is not an actor that makes calls")
            })?;
        let invocation = SorobanAuthorizedInvocation {
            function: SorobanAuthorizedFunction::ContractFn(InvokeContractArgs {
                contract_address: target.clone(),
                function_name: ScSymbol(
                    fn_name
                        .try_into()
                        .map_err(|_| format!("{fn_name:?} is not a symbol"))?,
                ),
                args: args
                    .to_vec()
                    .try_into()
                    .map_err(|_| String::from("too many arguments"))?,
            }),
            sub_invocations: VecM::default(),
        };
        self.nonce += 1;
        let nonce = self.nonce;
        let expiration = self
            .env
            .ledger()
            .sequence()
            .checked_add(100)
            .ok_or_else(|| String::from("the ledger sequence ran out"))?;
        let preimage = HashIdPreimage::SorobanAuthorization(HashIdPreimageSorobanAuthorization {
            network_id: Hash(self.env.ledger().network_id().to_array()),
            nonce,
            signature_expiration_ledger: expiration,
            invocation: invocation.clone(),
        });
        let payload = sha256(
            &preimage
                .to_xdr(Limits::none())
                .map_err(|e| format!("cannot encode the signature payload: {e}"))?,
        );
        let signature = match (&actor.key, &actor.smart) {
            (_, Some(wallet)) => smart::signature(
                &self.env,
                &to_address(&self.env, &actor.address)?,
                wallet,
                payload,
                count.unwrap_or(wallet.threshold),
            )?,
            (Some(key), None) => {
                let sig = key.sign(&payload).to_bytes();
                let entry = ScMap(
                    vec![
                        ScMapEntry {
                            key: scval::symbol("public_key")?,
                            val: bytes(&key.verifying_key().to_bytes())?,
                        },
                        ScMapEntry {
                            key: scval::symbol("signature")?,
                            val: bytes(&sig)?,
                        },
                    ]
                    .try_into()
                    .map_err(|_| String::from("cannot build the signature"))?,
                );
                sc_vec(vec![ScVal::Map(Some(entry))])?
            }
            (None, None) => ScVal::Void,
        };
        Ok(SorobanAuthorizationEntry {
            credentials: SorobanCredentials::Address(SorobanAddressCredentials {
                address: actor.address.clone(),
                nonce,
                signature_expiration_ledger: expiration,
                signature,
            }),
            root_invocation: invocation,
        })
    }

    fn invoke(
        &mut self,
        target: &ScAddress,
        fn_name: &str,
        args: &[ScVal],
        auth: Auth,
    ) -> Result<(Outcome, Vec<xdr::ContractEvent>)> {
        match auth {
            Auth::None => self.env.set_auths(&[]),
            Auth::Mock => self.env.mock_all_auths(),
            Auth::Signer(name) => {
                let entry = self.auth_entry(&name, target, fn_name, args, None)?;
                self.env.set_auths(&[entry]);
            }
            Auth::SignerWith(name, count) => {
                let entry = self.auth_entry(&name, target, fn_name, args, Some(count))?;
                self.env.set_auths(&[entry]);
            }
        }
        let outcome = call(&self.env, target, fn_name, args)?;
        let events = self.env.events().all().events().to_vec();
        Ok((outcome, events))
    }

    fn outcome_text(&self, o: &Outcome) -> String {
        match o {
            Outcome::Ok(v) => format!("returned {v:?}"),
            Outcome::Contract(c) => format!(
                "error {}",
                self.spec
                    .error_name(*c)
                    .map_or_else(|| c.to_string(), String::from)
            ),
            Outcome::Escaped { origin, code } => format!(
                "error {code} from {}",
                self.name_of(origin)
                    .unwrap_or_else(|| String::from("another contract"))
            ),
            Outcome::HostAbort(t) | Outcome::AuthFailure(t) | Outcome::HostError(t) => {
                format!("host error {t}")
            }
            Outcome::Panic(t) => format!("the test host panicked: {t}"),
        }
    }

    /// Runs a setup operation that must succeed.
    fn must(
        &mut self,
        what: &str,
        target: &ScAddress,
        fn_name: &str,
        args: &[ScVal],
        auth: Auth,
    ) -> Result<(Vec<Value>, Vec<Value>)> {
        let (o, events) = self.invoke(target, fn_name, args, auth)?;
        if !matches!(o, Outcome::Ok(_)) {
            return Err(format!("{what} failed: {}", self.outcome_text(&o)));
        }
        let (evs, toks, errs) = self.decode_events(&events);
        if let Some(e) = errs.first() {
            return Err(format!("{what}: {e}"));
        }
        Ok((evs, toks))
    }

    fn view(&mut self, target: &ScAddress, fn_name: &str, args: &[ScVal]) -> Result<Value> {
        let (o, _) = self.invoke(target, fn_name, args, Auth::None)?;
        match o {
            Outcome::Ok(v) => Ok(unwrap_enum(self.decode(&v, "")?)),
            other => Err(format!("{fn_name}() failed: {}", self.outcome_text(&other))),
        }
    }

    /// Runs the vector's setup in the format's order: create, allowlist,
    /// mints, transfers. None of it may fail.
    pub fn setup(&mut self, v: &Vector) -> Result<()> {
        let setup = v.setup().clone();
        if let Some(cfg) = setup.get("mandate").filter(|c| c.is_object()) {
            let principal = str_of(&cfg["principal"], "setup.mandate.principal")?.to_string();
            let salt = setup["salt"].as_str().unwrap_or("#salt");
            let args = vec![self.config_val(cfg)?, bytes(&bytes32(salt)?)?];
            let factory = self.factory.clone();
            let (o, _) = self.invoke(&factory, "create", &args, Auth::Signer(principal.clone()))?;
            match o {
                Outcome::Ok(ScVal::Address(a)) => self.set_mandate(a)?,
                other => {
                    return Err(format!(
                        "the setup could not create the mandate: {}",
                        self.outcome_text(&other)
                    ))
                }
            }
            if let Some(list) = setup["allowlist"].as_array().filter(|l| !l.is_empty()) {
                let entries = list
                    .iter()
                    .map(|e| self.allow_entry_val(e))
                    .collect::<Result<Vec<_>>>()?;
                let mandate = self.address_of("mandate")?;
                self.must(
                    "the setup allowlist",
                    &mandate,
                    "allow_recipients",
                    &[sc_vec(entries)?],
                    Auth::Signer(principal),
                )?;
            }
        }
        for m in setup["mints"].as_array().into_iter().flatten() {
            self.mint(
                str_of(&m["token"], "mint token")?,
                str_of(&m["to"], "mint to")?,
                parse_amount(&m["amount"], "mint amount")?,
            )?;
        }
        for t in setup["transfers"].as_array().into_iter().flatten() {
            self.transfer(
                str_of(&t["token"], "transfer token")?,
                str_of(&t["from"], "transfer from")?,
                str_of(&t["to"], "transfer to")?,
                parse_amount(&t["amount"], "transfer amount")?,
            )?;
        }
        Ok(())
    }

    pub fn mint(&mut self, token: &str, to: &str, amount: i128) -> Result<()> {
        let tok = self.token(token)?.address.clone();
        let args = [ScVal::Address(self.address_of(to)?), i128_val(amount)];
        self.must(
            &format!("minting {amount} {token} to {to}"),
            &tok,
            "mint",
            &args,
            Auth::Mock,
        )?;
        Ok(())
    }

    /// A plain token transfer. Returns the mandate and factory events and the
    /// token events it caused.
    pub fn transfer(
        &mut self,
        token: &str,
        from: &str,
        to: &str,
        amount: i128,
    ) -> Result<(Vec<Value>, Vec<Value>)> {
        let tok = self.token(token)?.address.clone();
        let args = [
            ScVal::Address(self.address_of(from)?),
            ScVal::Address(self.address_of(to)?),
            i128_val(amount),
        ];
        self.must(
            &format!("transferring {amount} {token} from {from} to {to}"),
            &tok,
            "transfer",
            &args,
            Auth::Mock,
        )
    }

    /// The asset's issuer takes `amount` from `from`.
    pub fn clawback(&mut self, from: &str, amount: i128) -> Result<()> {
        let tok = self.asset.address.clone();
        let args = [ScVal::Address(self.address_of(from)?), i128_val(amount)];
        self.must(
            &format!("clawing back {amount} from {from}"),
            &tok,
            "clawback",
            &args,
            Auth::Mock,
        )?;
        Ok(())
    }

    /// Whether an account can receive the settlement asset.
    pub fn set_receiving(&mut self, account_name: &str, state: &str) -> Result<()> {
        let pk = match self.actors.get(account_name).and_then(|a| a.key.as_ref()) {
            Some(k) => k.verifying_key().to_bytes(),
            None => {
                return Err(format!(
                    "{account_name} is not an account, so it holds no trustline"
                ))
            }
        };
        let asset = self
            .asset
            .asset
            .clone()
            .ok_or_else(|| String::from("the asset is not a Stellar asset"))?;
        match state {
            "receivable" => {
                ledger::set_trustline(&self.env, pk, &asset, ledger::AUTHORIZED | ledger::CLAWBACK)
            }
            "deauthorized" => ledger::set_trustline(&self.env, pk, &asset, ledger::CLAWBACK),
            "no_trustline" => match ledger::trustline(&self.env, pk, &asset)? {
                None => Ok(()),
                Some(t) if t.balance != 0 => Err(format!(
                    "{account_name} holds {} of the asset, and a trustline is only removed at zero",
                    t.balance
                )),
                Some(_) => self.drop_entry(&ledger::trustline_key(pk, &asset)),
            },
            other => Err(format!("unknown receiving state {other:?}")),
        }
    }

    /// The test host has no call that deletes a ledger entry, so the env is
    /// rebuilt from a snapshot of its ledger without that entry.
    fn drop_entry(&mut self, key: &LedgerKey) -> Result<()> {
        let mut snapshot = self.env.to_ledger_snapshot();
        let before = snapshot.ledger_entries.len();
        snapshot.ledger_entries.retain(|(k, _)| **k != *key);
        if snapshot.ledger_entries.len() == before {
            return Err(String::from("the entry to remove is not in the ledger"));
        }
        let info = self.env.ledger().get();
        let mut env = Env::from_ledger_snapshot(snapshot);
        env.set_config(EnvTestConfig {
            capture_snapshot_at_drop: false,
        });
        lift_host_limits(&env);
        env.ledger().set(info);
        // Native contracts live in the host, not in the ledger, so the mock
        // account contracts of contract actors are registered again. A smart
        // account is wasm and lives in the ledger.
        for a in self
            .actors
            .values()
            .filter(|a| a.key.is_none() && a.smart.is_none())
        {
            let address = to_address(&env, &a.address)?;
            if a.calls_back {
                env.register_at(&address, smart::CallsBackAccount, ());
            } else {
                env.register_at(&address, MockAuthContract, ());
            }
        }
        self.env = env;
        Ok(())
    }

    fn move_time(&mut self, to: u64) -> Result<()> {
        let info = self.env.ledger().get();
        if to < info.timestamp {
            return Err(format!(
                "time cannot move back from {} to {to}",
                info.timestamp
            ));
        }
        let ledgers = u32::try_from((to - info.timestamp).div_ceil(LEDGER_SECONDS))
            .ok()
            .and_then(|n| info.sequence_number.checked_add(n))
            .ok_or_else(|| String::from("the ledger sequence ran out"))?;
        self.env.ledger().with_mut(|li| {
            li.timestamp = to;
            li.sequence_number = ledgers;
        });
        Ok(())
    }

    fn destination_parts(&self, d: &Value) -> Result<(ScAddress, Option<u64>)> {
        let base = self.address_of(str_of(&d["address"], "destination address")?)?;
        let tag = match &d["tag"] {
            Value::Null => None,
            Value::String(_) => Some(parse_u64(&d["tag"], "tag")?),
            _ => {
                return Err(String::from(
                    "a tag marked absent that carries a value cannot be expressed on Stellar",
                ))
            }
        };
        if tag.is_some() && !matches!(base, ScAddress::Account(_)) {
            return Err(String::from("only an account takes a tag"));
        }
        Ok((base, tag))
    }

    fn destination_val(&self, d: &Value) -> Result<ScVal> {
        let (base, tag) = self.destination_parts(d)?;
        Ok(match (base, tag) {
            (
                ScAddress::Account(xdr::AccountId(xdr::PublicKey::PublicKeyTypeEd25519(pk))),
                Some(t),
            ) => ScVal::Address(scval::muxed(pk.0, t)),
            (b, _) => ScVal::Address(b),
        })
    }

    fn attester_bytes(&self, v: &Value) -> Result<Vec<u8>> {
        if let Some(actor) = v["account_key"].as_str() {
            return Ok(actor_key(actor).verifying_key().to_bytes().to_vec());
        }
        let s = str_of(v, "attester")?;
        if is_label(s) {
            return Ok(label_key(s)?.verifying_key().to_bytes().to_vec());
        }
        parse_hex(s)
    }

    fn addresses(&self, v: &Value) -> Result<ScVal> {
        let list = v
            .as_array()
            .ok_or_else(|| format!("{v} is not a list of actors"))?
            .iter()
            .map(|a| Ok(ScVal::Address(self.address_of(str_of(a, "actor")?)?)))
            .collect::<Result<Vec<_>>>()?;
        sc_vec(list)
    }

    fn limits_val(&self, l: &Value) -> Result<ScVal> {
        let amt = |k: &str| parse_amount(&l[k], k).map(i128_val);
        record(vec![
            ("total_budget", amt("total_budget")?),
            ("per_payment_cap", amt("per_payment_cap")?),
            ("per_recipient_daily_cap", amt("per_recipient_daily_cap")?),
            ("approval_threshold", amt("approval_threshold")?),
            (
                "payout_ttl_seconds",
                ScVal::U64(parse_u64(&l["payout_ttl_seconds"], "payout_ttl_seconds")?),
            ),
            (
                "expires_at",
                ScVal::U64(parse_u64(&l["expires_at"], "expires_at")?),
            ),
        ])
    }

    fn config_val(&self, c: &Value) -> Result<ScVal> {
        let mode = str_of(&c["recipient_mode"], "recipient_mode")?;
        let attester = if c["attester"].is_null() {
            ScVal::Void
        } else {
            bytes(&self.attester_bytes(&c["attester"])?)?
        };
        record(vec![
            (
                "principal",
                ScVal::Address(self.address_of(str_of(&c["principal"], "principal")?)?),
            ),
            (
                "asset",
                ScVal::Address(self.token(str_of(&c["asset"], "asset")?)?.address.clone()),
            ),
            (
                "recipient_mode",
                ScVal::U32(
                    mode_code(mode).ok_or_else(|| format!("unknown recipient mode {mode}"))?,
                ),
            ),
            (
                "tenant_tag",
                bytes(&bytes32(str_of(&c["tenant_tag"], "tenant_tag")?)?)?,
            ),
            ("agents", self.addresses(&c["agents"])?),
            ("approvers", self.addresses(&c["approvers"])?),
            (
                "approvals_required",
                ScVal::U32(parse_u32(&c["approvals_required"], "approvals_required")?),
            ),
            ("guardians", self.addresses(&c["guardians"])?),
            ("executors", self.addresses(&c["executors"])?),
            ("recovery", self.addresses(&c["recovery"])?),
            (
                "recovery_threshold",
                ScVal::U32(parse_u32(&c["recovery_threshold"], "recovery_threshold")?),
            ),
            ("limits", self.limits_val(&c["limits"])?),
            ("attester", attester),
            (
                "country_policy_hash",
                bytes(&bytes32(str_of(
                    &c["country_policy_hash"],
                    "country_policy_hash",
                )?)?)?,
            ),
            (
                "max_fee_bps",
                ScVal::U32(parse_u32(&c["max_fee_bps"], "max_fee_bps")?),
            ),
        ])
    }

    fn b32(&self, v: &Value, what: &str) -> Result<ScVal> {
        bytes(&bytes32(str_of(v, what)?)?)
    }

    fn request_val(&self, r: &Value) -> Result<ScVal> {
        let legs = r["legs"]
            .as_array()
            .ok_or_else(|| String::from("a request without legs"))?
            .iter()
            .map(|l| {
                record(vec![
                    ("destination", self.destination_val(&l["destination"])?),
                    (
                        "amount",
                        i128_val(parse_amount(&l["amount"], "leg amount")?),
                    ),
                ])
            })
            .collect::<Result<Vec<_>>>()?;
        let attestations = r["attestations"]
            .as_array()
            .ok_or_else(|| String::from("a request without attestations"))?
            .iter()
            .map(|n| {
                let name = str_of(n, "attestation")?;
                let a = self
                    .attestations
                    .get(name)
                    .ok_or_else(|| format!("no attest step made {name}"))?;
                record(vec![
                    ("valid_until", ScVal::U64(a.valid_until)),
                    ("policy_epoch", ScVal::U64(a.policy_epoch)),
                    ("signature", bytes(&a.signature)?),
                ])
            })
            .collect::<Result<Vec<_>>>()?;
        let fee = r
            .get("fee")
            .cloned()
            .unwrap_or_else(|| Value::String(String::from("0")));
        record(vec![
            ("payout_id", self.b32(&r["payout_id"], "payout_id")?),
            (
                "recipient_ref",
                self.b32(&r["recipient_ref"], "recipient_ref")?,
            ),
            ("total", i128_val(parse_amount(&r["total"], "total")?)),
            ("fee", i128_val(parse_amount(&fee, "fee")?)),
            ("legs", sc_vec(legs)?),
            (
                "deadline",
                ScVal::U64(parse_u64(&r["deadline"], "deadline")?),
            ),
            ("task_ref", self.b32(&r["task_ref"], "task_ref")?),
            ("attestations", sc_vec(attestations)?),
        ])
    }

    fn allow_entry_val(&self, e: &Value) -> Result<ScVal> {
        record(vec![
            (
                "recipient_ref",
                self.b32(&e["recipient_ref"], "recipient_ref")?,
            ),
            ("destination", self.destination_val(&e["destination"])?),
        ])
    }

    /// The contract and the arguments of a call step.
    fn call_args(&self, f: &str, caller: &str, a: &Value) -> Result<(ScAddress, Vec<ScVal>)> {
        if f == "create" {
            return Ok((
                self.factory.clone(),
                vec![
                    self.config_val(&a["config"])?,
                    self.b32(&a["salt"], "salt")?,
                ],
            ));
        }
        let mut out = Vec::new();
        if CALLER_ARG.contains(&f) {
            out.push(ScVal::Address(self.address_of(caller)?));
        }
        let list = |key: &str| -> Result<&Vec<Value>> {
            a[key]
                .as_array()
                .ok_or_else(|| format!("{f} needs a list {key}"))
        };
        match f {
            "request_payout" => out.push(self.request_val(&a["req"])?),
            "request_payouts" => out.push(sc_vec(
                list("reqs")?
                    .iter()
                    .map(|r| self.request_val(r))
                    .collect::<Result<Vec<_>>>()?,
            )?),
            "approve" | "reject" | "cancel" | "execute" | "payout" => {
                out.push(self.b32(&a["payout_id"], "payout_id")?)
            }
            "cancel_batch" | "execute_batch" => out.push(sc_vec(
                list("ids")?
                    .iter()
                    .map(|i| self.b32(i, "payout_id"))
                    .collect::<Result<Vec<_>>>()?,
            )?),
            "recovery_rotate_agent" => {
                out.push(ScVal::Address(self.address_of(str_of(&a["old"], "old")?)?));
                out.push(match &a["new"] {
                    Value::Null => ScVal::Void,
                    n => ScVal::Address(self.address_of(str_of(n, "new")?)?),
                });
            }
            "withdraw" => out.push(i128_val(parse_amount(&a["amount"], "amount")?)),
            "recover_token" => {
                out.push(ScVal::Address(
                    self.token(str_of(&a["token"], "token")?)?.address.clone(),
                ));
                out.push(i128_val(parse_amount(&a["amount"], "amount")?));
            }
            "update_limits" => out.push(self.limits_val(&a["limits"])?),
            "add_role" | "remove_role" => {
                out.push(ScVal::U32(parse_u32(&a["role"], "role")?));
                out.push(ScVal::Address(
                    self.address_of(str_of(&a["account"], "account")?)?,
                ));
            }
            "set_approvals_required" | "set_recovery_threshold" => {
                out.push(ScVal::U32(parse_u32(&a["n"], "n")?))
            }
            "allow_recipients" | "disallow_recipients" => out.push(sc_vec(
                list("entries")?
                    .iter()
                    .map(|e| self.allow_entry_val(e))
                    .collect::<Result<Vec<_>>>()?,
            )?),
            "set_attestation_policy" => {
                out.push(bytes(&self.attester_bytes(&a["attester"])?)?);
                out.push(self.b32(&a["country_policy_hash"], "country_policy_hash")?);
            }
            "propose_principal" => out.push(ScVal::Address(
                self.address_of(str_of(&a["proposed"], "proposed")?)?,
            )),
            "lower_fee_cap" => out.push(ScVal::U32(parse_u32(&a["new_bps"], "new_bps")?)),
            "daily_used" => out.push(self.b32(&a["recipient_ref"], "recipient_ref")?),
            "is_allowed" => {
                out.push(self.b32(&a["recipient_ref"], "recipient_ref")?);
                out.push(self.destination_val(&a["destination"])?);
            }
            "pause"
            | "sweep"
            | "recovery_revoke"
            | "resume"
            | "revoke"
            | "accept_principal"
            | "cancel_principal_proposal"
            | "version"
            | "config"
            | "state" => {}
            other => return Err(format!("unknown function {other}")),
        }
        Ok((self.address_of("mandate")?, out))
    }

    /// A call's return value in the vector's written form.
    fn decode_return(&self, f: &str, v: &ScVal) -> Result<Value> {
        match f {
            "request_payout" | "approve" => self.decode(v, "status"),
            "create"
            | "request_payouts"
            | "cancel_batch"
            | "revoke"
            | "sweep"
            | "recovery_revoke"
            | "recovery_rotate_agent"
            | "version"
            | "config"
            | "state"
            | "daily_used"
            | "is_allowed" => self.decode(v, ""),
            "payout" => Ok(unwrap_enum(self.decode(v, "")?)),
            _ if *v == ScVal::Void => Ok(Value::Null),
            _ => Err(format!(
                "{f} returned {v:?}, where the interface returns nothing"
            )),
        }
    }

    fn result_json(&self, f: &str, o: &Outcome) -> (Value, Vec<String>) {
        let mut errors = Vec::new();
        let result = match o {
            Outcome::Ok(v) => match self.decode_return(f, v) {
                Ok(r) => json!({ "ok": true, "return": r }),
                Err(e) => {
                    errors.push(e);
                    json!({ "ok": true, "return": null })
                }
            },
            Outcome::Contract(c) => match self.spec.error_name(*c) {
                Some(name) => json!({ "error": name }),
                None => {
                    errors.push(format!("contract error {c} is not in errors.json"));
                    json!({ "error": format!("CONTRACT_ERROR_{c}") })
                }
            },
            // Never written as a mandate error: the number is the other
            // contract's and may collide with one of ours.
            Outcome::Escaped { origin, code } => json!({
                "escaped_error": {
                    "from": self.name_of(origin).unwrap_or_else(|| String::from("unknown")),
                    "code": code,
                }
            }),
            Outcome::AuthFailure(_) => json!({ "host_auth_failure": true }),
            Outcome::HostAbort(t) => json!({ "host_abort": t }),
            Outcome::HostError(t) => json!({ "host_error": t }),
            Outcome::Panic(t) => json!({ "host_panic": t }),
        };
        (result, errors)
    }

    /// Splits events into the mandate's and the factory's, in canonical
    /// form, and the tokens'.
    fn decode_events(
        &self,
        events: &[xdr::ContractEvent],
    ) -> (Vec<Value>, Vec<Value>, Vec<String>) {
        let mut ours = Vec::new();
        let mut tokens = Vec::new();
        let mut errors = Vec::new();
        for ev in events {
            let source = ev
                .contract_id
                .as_ref()
                .and_then(|c| self.name_of(&ScAddress::Contract(c.clone())));
            let xdr::ContractEventBody::V0(body) = &ev.body;
            let topics: Vec<ScVal> = body.topics.iter().cloned().collect();
            match source.as_deref() {
                Some(t @ ("asset" | "other_token")) => {
                    let name = match topics.first() {
                        Some(ScVal::Symbol(s)) => s.0.to_utf8_string_lossy(),
                        _ => String::from("?"),
                    };
                    tokens.push(json!({ "token": t, "name": name }));
                }
                Some("factory" | "mandate") => match self.canonical_event(&topics, &body.data) {
                    Ok(e) => ours.push(e),
                    Err(e) => errors.push(e),
                },
                // A wallet's own events, and its policy's, are not the
                // mandate's and are not compared.
                Some(n)
                    if WALLET_CONTRACTS.contains(&n)
                        || self.actors.get(n).is_some_and(|a| a.smart.is_some()) => {}
                other => errors.push(format!(
                    "an event from {}, which is neither the factory, the mandate nor a token",
                    other.unwrap_or("an unknown contract")
                )),
            }
        }
        (ours, tokens, errors)
    }

    fn canonical_event(&self, topics: &[ScVal], data: &ScVal) -> Result<Value> {
        let name = match topics.first() {
            Some(ScVal::Symbol(s)) => s.0.to_utf8_string_lossy(),
            _ => return Err(String::from("an event whose first topic is not its name")),
        };
        if topics.len() != 2 {
            return Err(format!(
                "event {name} has {} topics; the interface gives every event two",
                topics.len()
            ));
        }
        let mut fields = match data {
            ScVal::Void => Map::new(),
            ScVal::Map(_) => match self.decode(data, "")? {
                Value::Object(o) => o,
                _ => Map::new(),
            },
            other => {
                return Err(format!(
                    "event {name} carries {other:?}, not a map of fields"
                ))
            }
        };
        // The key is carried only as a topic; it is a field when the event
        // lists it.
        if let Some(es) = self.spec.event(&name) {
            if es.fields.contains(&es.key) {
                fields.insert(es.key.clone(), self.decode(&topics[1], &es.key)?);
            }
            // A contract event leaves an Option field that is None out of
            // its data map, so a listed field that is absent is null. A
            // field that must have a value still fails the comparison, as
            // null never equals it.
            for f in &es.fields {
                if !fields.contains_key(f) {
                    fields.insert(f.clone(), Value::Null);
                }
            }
        }
        Ok(json!({ "name": name, "fields": fields }))
    }

    fn balance_of(&mut self, token: &Token, holder: &ScAddress) -> Result<String> {
        let (o, _) = self.invoke(
            &token.address,
            "balance",
            &[ScVal::Address(holder.clone())],
            Auth::None,
        )?;
        match o {
            Outcome::Ok(ScVal::I128(p)) => Ok(scval::i128_of(&p).to_string()),
            // An account with no trustline holds none of the asset.
            Outcome::Contract(SAC_TRUSTLINE_MISSING) if token.asset.is_some() => {
                Ok(String::from("0"))
            }
            other => Err(format!("balance() failed: {}", self.outcome_text(&other))),
        }
    }

    /// The state after a step, in the shape of a vector's `state`: the
    /// mandate's views, every payout and recipient the vector names, and
    /// every holder's balance of both tokens. Null before the mandate
    /// exists.
    pub fn capture_state(&mut self) -> (Value, Vec<String>) {
        let Some(mandate) = self.mandate.clone() else {
            return (Value::Null, Vec::new());
        };
        let mut errors = Vec::new();
        let mut m = Map::new();
        for (view, fields) in [("state", &STATE_FIELDS[..]), ("config", &CONFIG_FIELDS[..])] {
            match self.view(&mandate, view, &[]) {
                Ok(Value::Object(o)) => {
                    for k in fields {
                        if let Some(v) = o.get(*k) {
                            m.insert((*k).to_string(), v.clone());
                        }
                    }
                }
                Ok(other) => errors.push(format!("{view}() returned {other}, not a record")),
                Err(e) => errors.push(e),
            }
        }
        let mut payouts = Map::new();
        for id in self.payout_ids.clone() {
            let key = self.bytes32(&id);
            match bytes(&id).and_then(|b| self.view(&mandate, "payout", &[b])) {
                Ok(v) => {
                    payouts.insert(key, v);
                }
                Err(e) => errors.push(e),
            }
        }
        let mut daily = Map::new();
        for r in self.refs.clone() {
            let key = self.bytes32(&r);
            match bytes(&r).and_then(|b| self.view(&mandate, "daily_used", &[b])) {
                Ok(v) => {
                    daily.insert(key, v);
                }
                Err(e) => errors.push(e),
            }
        }
        let mut holders: Vec<(String, ScAddress)> = self
            .actors
            .iter()
            .map(|(n, a)| (n.clone(), a.address.clone()))
            .collect();
        holders.push((String::from("mandate"), mandate.clone()));
        let mut balances = Map::new();
        for (name, token) in [
            ("asset", self.asset.clone()),
            ("other_token", self.other.clone()),
        ] {
            let mut per = Map::new();
            for (holder, addr) in &holders {
                match self.balance_of(&token, addr) {
                    Ok(b) => {
                        per.insert(holder.clone(), Value::String(b));
                    }
                    Err(e) => errors.push(format!("{name} balance of {holder}: {e}")),
                }
            }
            balances.insert(name.to_string(), Value::Object(per));
        }
        (
            json!({ "mandate": m, "payouts": payouts, "balances": balances, "daily": daily }),
            errors,
        )
    }

    fn attest(&mut self, step: &Value) -> Result<Value> {
        let mandate = self.address_of("mandate")?;
        let name = str_of(&step["name"], "attestation name")?.to_string();
        let signer = label_key(str_of(&step["signer"], "signer")?)?;
        let wrong = label_key("#wrong_signer")?;
        let cfg = self.view(&mandate, "config", &[])?;
        let policy = bytes32(str_of(
            &cfg["country_policy_hash"],
            "config().country_policy_hash",
        )?)?;
        let policy_epoch = match step.get("policy_epoch") {
            Some(e) => parse_u64(e, "policy_epoch")?,
            None => {
                let st = self.view(&mandate, "state", &[])?;
                parse_u64(&st["policy_epoch"], "state().policy_epoch")?
            }
        };
        let (base, tag) = self.destination_parts(&step["destination"])?;
        let input = AttestInput {
            network_id: self.env.ledger().network_id().to_array(),
            mandate: address_xdr(&mandate)?,
            recipient_ref: bytes32(str_of(&step["recipient_ref"], "recipient_ref")?)?,
            country_policy_hash: policy,
            policy_epoch,
            valid_until: parse_u64(&step["valid_until"], "valid_until")?,
            destination: address_xdr(&base)?,
            tag,
        };
        let tamper = step["tamper"].as_str().map(Tamper::parse).transpose()?;
        let (_, att) = attest::attest(&signer, &wrong, &input, tamper)?;
        let out = json!({
            "name": name,
            "valid_until": att.valid_until.to_string(),
            "policy_epoch": att.policy_epoch.to_string(),
            "signature": hex(&att.signature),
        });
        self.attestations.insert(name, att);
        Ok(out)
    }

    /// Runs one step and returns its trace record, and whether the run must
    /// stop because the test host panicked.
    pub fn step(&mut self, index: usize, step: &Value) -> Result<(Value, bool)> {
        let mut rec = step.as_object().cloned().unwrap_or_default();
        rec.remove("expect");
        rec.insert(String::from("index"), Value::from(index));
        let mut stop = false;
        match step["step"].as_str().unwrap_or("") {
            "call" => {
                let f = str_of(&step["fn"], "fn")?.to_string();
                let caller = str_of(&step["caller"], "caller")?.to_string();
                let signer = step["signed_by"].as_str().unwrap_or(&caller).to_string();
                let (target, args) = self.call_args(&f, &caller, &step["args"])?;
                let auth = if VIEWS.contains(&f.as_str()) {
                    Auth::None
                } else if let Some(n) = step["signatures"].as_u64() {
                    let n = u32::try_from(n).map_err(|_| String::from("too many signatures"))?;
                    Auth::SignerWith(signer, n)
                } else {
                    Auth::Signer(signer)
                };
                let (o, events) = self.invoke(&target, &f, &args, auth)?;
                self.last_call = last_invocation(&self.env);
                if let (Outcome::Ok(ScVal::Address(a)), "create", None) =
                    (&o, f.as_str(), &self.mandate)
                {
                    self.set_mandate(a.clone())?;
                }
                let (result, mut errors) = self.result_json(&f, &o);
                let (evs, toks, event_errors) = self.decode_events(&events);
                errors.extend(event_errors);
                rec.insert(String::from("result"), result);
                rec.insert(String::from("events"), Value::Array(evs));
                rec.insert(String::from("token_events"), Value::Array(toks));
                if !errors.is_empty() {
                    rec.insert(String::from("decode_errors"), json!(errors));
                }
                stop = matches!(o, Outcome::Panic(_));
            }
            "advance_time" => {
                let now = self.env.ledger().timestamp();
                let s = parse_u64(&step["seconds"], "seconds")?;
                self.move_time(
                    now.checked_add(s)
                        .ok_or_else(|| String::from("time passes u64"))?,
                )?;
            }
            "set_time" => self.move_time(parse_u64(&step["now"], "now")?)?,
            "transfer" => {
                let (evs, toks) = self.transfer(
                    str_of(&step["token"], "token")?,
                    str_of(&step["from"], "from")?,
                    str_of(&step["to"], "to")?,
                    parse_amount(&step["amount"], "amount")?,
                )?;
                rec.insert(String::from("events"), Value::Array(evs));
                rec.insert(String::from("token_events"), Value::Array(toks));
            }
            "mint" => self.mint(
                str_of(&step["token"], "token")?,
                str_of(&step["to"], "to")?,
                parse_amount(&step["amount"], "amount")?,
            )?,
            "attest" => {
                let a = self.attest(step)?;
                rec.insert(String::from("attestation"), a);
            }
            "set_receiving" => self.set_receiving(
                str_of(&step["account"], "account")?,
                str_of(&step["state"], "state")?,
            )?,
            "clawback" => self.clawback(
                str_of(&step["from"], "from")?,
                parse_amount(&step["amount"], "amount")?,
            )?,
            other => return Err(format!("unknown step kind {other:?}")),
        }
        rec.insert(
            String::from("now"),
            Value::String(self.env.ledger().timestamp().to_string()),
        );
        let (state, errors) = self.capture_state();
        rec.insert(String::from("state"), state);
        if !errors.is_empty() {
            rec.insert(String::from("state_errors"), json!(errors));
        }
        Ok((Value::Object(rec), stop))
    }

    /// What the last call step used.
    pub fn call_resources(&self) -> Option<CallResources> {
        self.last_call
    }

    /// Each actor's strkey, for reading a trace next to a network.
    pub fn actor_strkeys(&self) -> Value {
        let mut out = Map::new();
        let mut all: Vec<(String, ScAddress)> = self
            .actors
            .iter()
            .map(|(n, a)| (n.clone(), a.address.clone()))
            .collect();
        all.push((String::from("asset"), self.asset.address.clone()));
        all.push((String::from("other_token"), self.other.address.clone()));
        all.push((String::from("factory"), self.factory.clone()));
        if let Some(m) = &self.mandate {
            all.push((String::from("mandate"), m.clone()));
        }
        for (name, a) in all {
            if let Ok(addr) = to_address(&self.env, &a) {
                let s = addr.to_string();
                let mut buf = vec![0u8; s.len() as usize];
                s.copy_into_slice(&mut buf);
                out.insert(
                    name,
                    Value::String(String::from_utf8_lossy(&buf).into_owned()),
                );
            }
        }
        Value::Object(out)
    }
}

/// A view may answer with a `#[contracttype]` enum around a struct, such as
/// a record or a tombstone; its fields are what the trace keeps.
fn unwrap_enum(v: Value) -> Value {
    if let Value::Array(items) = &v {
        if let [Value::String(_), inner @ Value::Object(_)] = items.as_slice() {
            return inner.clone();
        }
    }
    v
}

fn make_tokens(env: &Env, setup: &Value) -> Result<(Token, Token)> {
    let admin_key = SigningKey::from_bytes(&sha256(b"ujira-test-token-admin"));
    let admin = to_address(env, &account(admin_key.verifying_key().to_bytes()))?;
    let asset_kind = setup["tokens"]["asset"]["kind"]
        .as_str()
        .unwrap_or("standard");
    if asset_kind != "standard" {
        return Err(format!(
            "an asset of kind {asset_kind} cannot run on Stellar"
        ));
    }
    let sac = env.register_stellar_asset_contract_v2(admin.clone());
    // The issuer can freeze and claw back, as Circle's USDC issuer on
    // Stellar can, so the set_receiving and clawback steps have something to
    // act through. Set before any balance exists, so every balance inherits
    // the clawback flag.
    sac.issuer().set_flag(IssuerFlags::RevocableFlag);
    sac.issuer().set_flag(IssuerFlags::ClawbackEnabledFlag);
    let asset = Token {
        address: ScAddress::from(&sac.address()),
        asset: Some(sac.asset()),
    };
    let other_kind = setup["tokens"]["other_token"]["kind"]
        .as_str()
        .unwrap_or("standard");
    let other = match other_kind {
        "standard" => {
            let s = env.register_stellar_asset_contract_v2(admin);
            Token {
                address: ScAddress::from(&s.address()),
                asset: Some(s.asset()),
            }
        }
        "moves_settlement" | "returns_value" => {
            let kind: u32 = if other_kind == "moves_settlement" {
                1
            } else {
                2
            };
            let addr = env.register(TEST_TOKEN_WASM, (kind, sac.address()));
            Token {
                address: ScAddress::from(&addr),
                asset: None,
            }
        }
        k => return Err(format!("an other_token of kind {k} cannot run on Stellar")),
    };
    Ok((asset, other))
}

fn register_factory(
    env: &Env,
    contracts: &Contracts,
    asset: &ScAddress,
    fee_address: &ScAddress,
) -> Result<ScAddress> {
    let wasm_hash = env
        .deployer()
        .upload_contract_wasm(soroban_sdk::Bytes::from_slice(env, &contracts.mandate));
    let mut assets = soroban_sdk::Vec::<Address>::new(env);
    assets.push_back(to_address(env, asset)?);
    let mut args = soroban_sdk::Vec::<Val>::new(env);
    args.push_back(wasm_hash.into_val(env));
    args.push_back(assets.into_val(env));
    args.push_back(to_address(env, fee_address)?.into_val(env));
    let addr = env.register(contracts.factory.as_slice(), args);
    Ok(ScAddress::from(&addr))
}

/// Runs one vector and returns its trace entry.
pub fn run_vector(spec: &Spec, contracts: &Contracts, v: &Vector) -> Value {
    run_vector_as(spec, contracts, v, PrincipalMode::AsDeclared)
}

/// As `run_vector`, with the principal chosen by `mode`.
pub fn run_vector_as(spec: &Spec, contracts: &Contracts, v: &Vector, mode: PrincipalMode) -> Value {
    let mut world = match World::with_principal(spec, contracts, v, mode) {
        Ok(w) => w,
        Err(e) => {
            return crate::trace::unrun(&v.id, &v.file, status::ABORTED, &format!("setup: {e}"))
        }
    };
    let mut out = json!({ "id": v.id, "file": v.file, "status": status::RAN });
    if let Err(e) = world.setup(v) {
        out["status"] = Value::from(status::ABORTED);
        out["reason"] = Value::String(format!("setup: {e}"));
        out["actors"] = world.actor_strkeys();
        out["setup_state"] = Value::Null;
        out["steps"] = json!([]);
        return out;
    }
    let (setup_state, setup_errors) = world.capture_state();
    out["setup_state"] = setup_state;
    if !setup_errors.is_empty() {
        out["setup_state_errors"] = json!(setup_errors);
    }
    let mut steps = Vec::new();
    for (i, step) in v.steps().iter().enumerate() {
        match world.step(i + 1, step) {
            Ok((rec, stop)) => {
                steps.push(rec);
                if stop {
                    out["status"] = Value::from(status::ABORTED);
                    out["reason"] =
                        Value::String(format!("step {}: the test host panicked", i + 1));
                    break;
                }
            }
            Err(e) => {
                out["status"] = Value::from(status::ABORTED);
                out["reason"] = Value::String(format!("step {}: {e}", i + 1));
                break;
            }
        }
    }
    out["actors"] = world.actor_strkeys();
    out["steps"] = Value::Array(steps);
    out
}
