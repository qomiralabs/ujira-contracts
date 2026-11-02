// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Tests for the mandate. Every test runs the compiled wasm, registered by
//! `register_mandate_wasm`, never the native contract: the host reports some
//! errors differently through a native test contract than a network does.
//!
//! The wasm is read from the release folder that `scripts/soroban-build.sh`
//! fills, or from `UJIRA_MANDATE_WASM`. A wasm older than the contract's
//! source is refused, so the tests can never pass against yesterday's build.

extern crate std;

mod admin;
mod approval;
mod attestation;
mod attestation_properties;
mod cancel;
mod constructor;
mod execute;
mod fee_cap_property;
mod interface;
mod lifecycle;
mod recovery;
mod request;
mod storage_ttl;
mod views;

use crate::storage;
use crate::types::{Leg, PayoutEntry, PayoutRecord, PayoutRequest, Tombstone};
use crate::{Limits, MandateConfig, MandateError, UjiraMandateClient};
use soroban_sdk::{
    testutils::{Address as _, EnvTestConfig, Events as _, Ledger as _},
    token::StellarAssetClient,
    xdr::{self, ContractEventBody, ContractEventType, FromXdr, ScError, ScVal},
    Address, Bytes, BytesN, Env, MuxedAddress, Vec,
};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;

/// 2026-01-01 00:00:00 UTC, the time most vectors start at.
pub const NOW: u64 = 1_767_225_600;
pub const START_SEQUENCE: u32 = 1_000;
/// Testnet's archival settings, so entries live and archive as they do
/// there.
pub const MIN_PERSISTENT_TTL: u32 = 120_960;
pub const MAX_ENTRY_TTL: u32 = 3_110_400;
pub const DAY: u64 = 86_400;

fn wasm_path() -> PathBuf {
    match std::env::var_os("UJIRA_MANDATE_WASM") {
        Some(p) => PathBuf::from(p),
        None => Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/wasm32v1-none/release/ujira_mandate.wasm"),
    }
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

fn mandate_wasm() -> &'static [u8] {
    static WASM: OnceLock<std::vec::Vec<u8>> = OnceLock::new();
    WASM.get_or_init(|| {
        let path = wasm_path();
        let built = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .unwrap_or_else(|e| {
                panic!(
                    "{}: {e}; build the contracts first with scripts/soroban-build.sh",
                    path.display()
                )
            });
        // The tests are not part of the wasm, so editing them needs no
        // rebuild; anything else that goes into it does.
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let tests = manifest.join("src/test");
        let mut newest = SystemTime::UNIX_EPOCH;
        for dir in [
            manifest.join("src"),
            manifest.join("../../crates/mandate-config/src"),
            manifest.join("../../crates/mandate-errors/src"),
        ] {
            newest_source(&dir, &tests, &mut newest);
        }
        assert!(
            std::env::var_os("UJIRA_MANDATE_WASM").is_some() || built >= newest,
            "{} is older than the contract source; rebuild it with scripts/soroban-build.sh",
            path.display()
        );
        std::fs::read(&path).unwrap()
    })
}

/// Why a registration failed.
#[derive(Debug, Eq, PartialEq)]
pub enum CreateError {
    /// The constructor returned this mandate error.
    Mandate(MandateError),
    /// Anything else, such as a host error.
    Host,
}

/// The one way the tests create a mandate: the compiled wasm, constructed
/// with these arguments. Returns the constructor's error when it fails.
pub fn register_mandate_wasm(
    env: &Env,
    factory: &Address,
    fee_address: &Address,
    config: &MandateConfig,
) -> Result<Address, CreateError> {
    let wasm = mandate_wasm();
    let got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        env.register(wasm, (factory.clone(), fee_address.clone(), config.clone()))
    }));
    match got {
        Ok(addr) => Ok(addr),
        Err(_) => Err(match last_contract_error(env) {
            Some(code) => MandateError::try_from(soroban_sdk::Error::from_contract_error(code))
                .map_or(CreateError::Host, CreateError::Mandate),
            None => CreateError::Host,
        }),
    }
}

/// A failed constructor reaches the caller as a generic host error; its
/// own code is only in the diagnostic events.
fn last_contract_error(env: &Env) -> Option<u32> {
    let events = env.host().get_diagnostic_events().ok()?;
    let mut found = None;
    for he in events.0.iter() {
        let ev = &he.event;
        if ev.type_ != ContractEventType::Diagnostic {
            continue;
        }
        let ContractEventBody::V0(body) = &ev.body;
        let is_error =
            matches!(body.topics.first(), Some(ScVal::Symbol(s)) if s.0.as_slice() == b"error");
        if let (true, Some(ScVal::Error(ScError::Contract(code)))) = (is_error, body.topics.get(1))
        {
            found = Some(*code);
        }
    }
    found
}

/// A fresh test host with testnet's ledger settings and one actor per role.
pub struct Fx {
    pub env: Env,
    pub factory: Address,
    pub fee_address: Address,
    pub asset: Address,
    pub principal: Address,
    pub agent: Address,
    pub approver: Address,
    pub guardian: Address,
    pub executor: Address,
    pub recovery: Address,
}

impl Fx {
    pub fn new() -> Fx {
        let env = Env::new_with_config(EnvTestConfig {
            // Nothing here is a snapshot test.
            capture_snapshot_at_drop: false,
        });
        env.ledger().with_mut(|li| {
            li.timestamp = NOW;
            li.sequence_number = START_SEQUENCE;
            li.min_persistent_entry_ttl = MIN_PERSISTENT_TTL;
            li.min_temp_entry_ttl = 720;
            li.max_entry_ttl = MAX_ENTRY_TTL;
        });
        // Only minting into the mandate needs an authorization here.
        env.mock_all_auths();
        let issuer = account(&env, 99);
        let asset = env.register_stellar_asset_contract_v2(issuer).address();
        Fx {
            factory: Address::generate(&env),
            fee_address: account(&env, 7),
            asset,
            principal: account(&env, 1),
            agent: account(&env, 2),
            approver: account(&env, 3),
            guardian: account(&env, 4),
            executor: account(&env, 5),
            recovery: account(&env, 6),
            env,
        }
    }

    pub fn account(&self, seed: u8) -> Address {
        account(&self.env, seed)
    }

    pub fn list(&self, items: &[&Address]) -> Vec<Address> {
        let mut out = Vec::new(&self.env);
        for a in items {
            out.push_back((*a).clone());
        }
        out
    }

    pub fn b32(&self, byte: u8) -> BytesN<32> {
        BytesN::from_array(&self.env, &[byte; 32])
    }

    /// A configuration that passes every check and fills every role set.
    pub fn config(&self) -> MandateConfig {
        MandateConfig {
            principal: self.principal.clone(),
            asset: self.asset.clone(),
            recipient_mode: mandate_config::MODE_ALLOWLIST,
            tenant_tag: self.b32(0x7e),
            agents: self.list(&[&self.agent]),
            approvers: self.list(&[&self.approver]),
            approvals_required: 1,
            guardians: self.list(&[&self.guardian]),
            executors: self.list(&[&self.executor]),
            recovery: self.list(&[&self.recovery]),
            recovery_threshold: 1,
            limits: Limits {
                total_budget: 10_000,
                per_payment_cap: 1_000,
                per_recipient_daily_cap: 500,
                approval_threshold: 500,
                payout_ttl_seconds: DAY,
                expires_at: NOW + 365 * DAY,
            },
            attester: None,
            country_policy_hash: self.b32(0xc0),
            max_fee_bps: 0,
        }
    }

    pub fn create(&self, config: &MandateConfig) -> Result<Address, CreateError> {
        register_mandate_wasm(&self.env, &self.factory, &self.fee_address, config)
    }

    /// Creates the base configuration and returns a client for it.
    pub fn mandate(&self) -> UjiraMandateClient<'_> {
        let id = self.create(&self.config()).unwrap();
        UjiraMandateClient::new(&self.env, &id)
    }

    pub fn mint(&self, to: &Address, amount: i128) {
        StellarAssetClient::new(&self.env, &self.asset).mint(to, &amount);
    }
}

/// An account address whose ed25519 key is 32 bytes of `seed`, built from
/// its XDR: an address value, an account, an ed25519 key, then the key.
pub fn account(env: &Env, seed: u8) -> Address {
    let mut xdr = Bytes::from_array(env, &[0, 0, 0, 18, 0, 0, 0, 0, 0, 0, 0, 0]);
    xdr.append(&key_of(env, seed));
    Address::from_xdr(env, &xdr).unwrap()
}

/// The contract address whose 32-byte hash is all zeros: an address value,
/// a contract, then the hash.
pub fn zero_contract(env: &Env) -> Address {
    let mut xdr = Bytes::from_array(env, &[0, 0, 0, 18, 0, 0, 0, 1]);
    xdr.append(&Bytes::from_array(env, &[0; 32]));
    Address::from_xdr(env, &xdr).unwrap()
}

/// Every address a payout could be sent to without anyone ever being able
/// to spend it: the mandate itself, the settlement asset's contract, and
/// the account and the contract whose key is all zeros.
pub fn unpayable(fx: &Fx, mandate: &Address) -> [Address; 4] {
    [
        mandate.clone(),
        fx.asset.clone(),
        account(&fx.env, 0),
        zero_contract(&fx.env),
    ]
}

/// The ed25519 key of an account made by `account`.
pub fn key_of(env: &Env, seed: u8) -> Bytes {
    Bytes::from_array(env, &[seed; 32])
}

/// The recipient most tests pay, and the wallet bound to it.
pub const BOB: u8 = 0xb0;
pub const BOB_WALLET: u8 = 50;

/// What the mandate base configuration holds before any request.
pub const FUNDED: i128 = 10_000;

/// A destination: the account `seed`, with or without a tag.
pub fn dest(env: &Env, seed: u8, tag: Option<u64>) -> MuxedAddress {
    let base = account(env, seed);
    match tag {
        None => MuxedAddress::from(base),
        Some(id) => <MuxedAddress as soroban_sdk::testutils::MuxedAddress>::new(&base, id),
    }
}

/// The mandate error a refused call returned. Anything else, a success or
/// a host failure, fails the test.
pub fn code<T: core::fmt::Debug, C: core::fmt::Debug, I: core::fmt::Debug>(
    r: Result<Result<T, C>, Result<MandateError, I>>,
) -> MandateError {
    match r {
        Err(Ok(e)) => e,
        other => panic!("expected a mandate error, got {other:?}"),
    }
}

impl Fx {
    pub fn bob(&self) -> BytesN<32> {
        self.b32(BOB)
    }

    pub fn bob_wallet(&self) -> MuxedAddress {
        dest(&self.env, BOB_WALLET, None)
    }

    /// Binds a destination to a recipient straight in storage. The
    /// allowlist calls come later; until then this writes exactly the entry
    /// they will.
    pub fn allow(&self, m: &Address, recipient_ref: &BytesN<32>, destination: &MuxedAddress) {
        self.env.as_contract(m, || {
            storage::set_allowed(&self.env, recipient_ref, destination)
        });
    }

    /// The base mandate, funded and with Bob's wallet allowed.
    pub fn ready(&self) -> UjiraMandateClient<'_> {
        self.ready_with(&self.config())
    }

    pub fn ready_with(&self, config: &MandateConfig) -> UjiraMandateClient<'_> {
        let id = self.create(config).unwrap();
        self.mint(&id, FUNDED);
        self.allow(&id, &self.bob(), &self.bob_wallet());
        UjiraMandateClient::new(&self.env, &id)
    }

    /// A one-leg request to Bob's wallet, due ten minutes from now.
    pub fn req(&self, id: u8, total: i128) -> PayoutRequest {
        PayoutRequest {
            payout_id: self.b32(id),
            recipient_ref: self.bob(),
            total,
            fee: 0,
            legs: Vec::from_array(
                &self.env,
                [Leg {
                    destination: self.bob_wallet(),
                    amount: total,
                }],
            ),
            deadline: self.env.ledger().timestamp() + 600,
            task_ref: self.b32(0x7a),
            attestations: Vec::new(&self.env),
        }
    }

    pub fn record(&self, m: &UjiraMandateClient, id: u8) -> PayoutRecord {
        match m.payout(&self.b32(id)) {
            Some(PayoutEntry::Record(r)) => r,
            other => panic!("expected a record for {id}, got {other:?}"),
        }
    }

    pub fn tombstone(&self, m: &UjiraMandateClient, id: u8) -> Tombstone {
        match m.payout(&self.b32(id)) {
            Some(PayoutEntry::Tombstone(t)) => t,
            other => panic!("expected a tombstone for {id}, got {other:?}"),
        }
    }

    /// Edits the stored roles, as the role calls and recovery will.
    pub fn edit_roles(&self, m: &Address, edit: impl FnOnce(&mut storage::Roles)) {
        self.env.as_contract(m, || {
            let mut roles = storage::roles(&self.env).unwrap();
            edit(&mut roles);
            storage::set_roles(&self.env, &roles);
        });
    }

    pub fn edit_flags(&self, m: &Address, edit: impl FnOnce(&mut storage::Flags)) {
        self.env.as_contract(m, || {
            let mut flags = storage::flags(&self.env).unwrap();
            edit(&mut flags);
            storage::set_flags(&self.env, &flags);
        });
    }

    /// The mandate's own events from the last call.
    pub fn events(&self, m: &Address) -> std::vec::Vec<xdr::ContractEvent> {
        self.env
            .events()
            .all()
            .filter_by_contract(m)
            .events()
            .to_vec()
    }

    pub fn set_time(&self, now: u64) {
        self.env.ledger().with_mut(|li| li.timestamp = now);
    }
}
