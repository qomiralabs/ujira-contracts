// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Tests for the factory. The factory and the mandate it deploys are both
//! the compiled wasm, never native test contracts: a constructor error seen
//! through a native deploy is not what a network reports, and that
//! difference is exactly what the factory exists to hide.
//!
//! The wasm is read from the release folder that `scripts/soroban-build.sh`
//! fills, or from `UJIRA_FACTORY_WASM` and `UJIRA_MANDATE_WASM`. A wasm older
//! than its source is refused, so the tests never pass against an old build.

extern crate std;

mod factory;

use crate::UjiraFactoryClient;
use mandate_config::{Limits, MandateConfig, MODE_ALLOWLIST};
use mandate_errors::MandateError;
use soroban_sdk::{
    testutils::{EnvTestConfig, Ledger as _},
    xdr::{
        ContractEventBody, ContractEventType, ContractExecutable, ContractId, ContractIdPreimage,
        ContractIdPreimageFromAddress, Hash, HashIdPreimage, HashIdPreimageContractId, LedgerKey,
        LedgerKeyContractData, Limits as XdrLimits, ScAddress, ScError, ScVal, Uint256, WriteXdr,
    },
    Address, Bytes, BytesN, Env, TryFromVal, Vec,
};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::SystemTime;

/// 2026-01-01 00:00:00 UTC.
pub const NOW: u64 = 1_767_225_600;
pub const DAY: u64 = 86_400;

fn manifest() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
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

/// Reads a contract's release wasm, refusing one older than the sources
/// that go into it. Tests are never part of the wasm, so they are skipped.
fn load_wasm(var: &str, file: &str, contract_dir: &Path) -> std::vec::Vec<u8> {
    let path = match std::env::var_os(var) {
        Some(p) => PathBuf::from(p),
        None => manifest()
            .join("../../target/wasm32v1-none/release")
            .join(file),
    };
    let built = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .unwrap_or_else(|e| {
            panic!(
                "{}: {e}; build the contracts first with scripts/soroban-build.sh",
                path.display()
            )
        });
    let mut newest = SystemTime::UNIX_EPOCH;
    for dir in [
        contract_dir.join("src"),
        manifest().join("../../crates/mandate-config/src"),
        manifest().join("../../crates/mandate-errors/src"),
    ] {
        newest_source(&dir, &contract_dir.join("src/test"), &mut newest);
    }
    assert!(
        std::env::var_os(var).is_some() || built >= newest,
        "{} is older than the contract source; rebuild it with scripts/soroban-build.sh",
        path.display()
    );
    std::fs::read(&path).unwrap()
}

pub fn factory_wasm() -> &'static [u8] {
    static WASM: OnceLock<std::vec::Vec<u8>> = OnceLock::new();
    WASM.get_or_init(|| load_wasm("UJIRA_FACTORY_WASM", "ujira_factory.wasm", manifest()))
}

pub fn mandate_wasm() -> &'static [u8] {
    static WASM: OnceLock<std::vec::Vec<u8>> = OnceLock::new();
    WASM.get_or_init(|| {
        load_wasm(
            "UJIRA_MANDATE_WASM",
            "ujira_mandate.wasm",
            &manifest().join("../mandate"),
        )
    })
}

/// The contract error code of the last failed frame, which is the only place
/// a failed constructor's own code shows up.
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

/// Registers the compiled factory at `at`, or wherever the host puts it.
/// Returns the constructor's error when it refuses the arguments.
pub fn register_factory(
    env: &Env,
    at: Option<&Address>,
    wasm_hash: &BytesN<32>,
    assets: &Vec<Address>,
    fee_address: &Address,
) -> Result<Address, Option<MandateError>> {
    let args = (wasm_hash.clone(), assets.clone(), fee_address.clone());
    let got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match at {
        Some(addr) => {
            env.register_at(addr, factory_wasm(), args);
            addr.clone()
        }
        None => env.register(factory_wasm(), args),
    }));
    match got {
        Ok(addr) => Ok(addr),
        Err(_) => Err(last_contract_error(env).and_then(|code| {
            MandateError::try_from(soroban_sdk::Error::from_contract_error(code)).ok()
        })),
    }
}

/// An account address whose ed25519 key is 32 bytes of `seed`.
pub fn account(env: &Env, seed: u8) -> Address {
    address_of(env, account_sc(seed))
}

pub fn account_sc(seed: u8) -> ScAddress {
    ScAddress::Account(soroban_sdk::xdr::AccountId(
        soroban_sdk::xdr::PublicKey::PublicKeyTypeEd25519(Uint256([seed; 32])),
    ))
}

/// A contract address whose hash is 32 bytes of `seed`. Nothing is
/// deployed there.
pub fn contract(env: &Env, seed: u8) -> Address {
    address_of(env, ScAddress::Contract(ContractId(Hash([seed; 32]))))
}

fn address_of(env: &Env, sc: ScAddress) -> Address {
    Address::try_from_val(env, &sc).unwrap()
}

pub fn sha256(env: &Env, data: &[u8]) -> [u8; 32] {
    env.crypto()
        .sha256(&Bytes::from_slice(env, data))
        .to_array()
}

/// The address `create` deploys to, computed from the XDR the way an
/// off-chain client would, without asking any contract: the contract ID
/// preimage of the factory's address and the salt
/// `SHA-256(principal as an XDR ScVal address || salt)`, on this network.
pub fn predicted_address(
    env: &Env,
    factory: &Address,
    principal: &Address,
    salt: &BytesN<32>,
) -> Address {
    let mut seed = ScVal::Address(ScAddress::from(principal))
        .to_xdr(XdrLimits::none())
        .unwrap();
    seed.extend_from_slice(&salt.to_array());
    let preimage = HashIdPreimage::ContractId(HashIdPreimageContractId {
        network_id: Hash(env.ledger().network_id().to_array()),
        contract_id_preimage: ContractIdPreimage::Address(ContractIdPreimageFromAddress {
            address: ScAddress::from(factory),
            salt: Uint256(sha256(env, &seed)),
        }),
    });
    let id = sha256(env, &preimage.to_xdr(XdrLimits::none()).unwrap());
    address_of(env, ScAddress::Contract(ContractId(Hash(id))))
}

/// The code a deployed contract runs, or None when nothing is deployed at
/// the address.
pub fn executable_of(env: &Env, contract: &Address) -> Option<ContractExecutable> {
    let key = LedgerKey::ContractData(LedgerKeyContractData {
        contract: ScAddress::from(contract),
        key: ScVal::LedgerKeyContractInstance,
        durability: soroban_sdk::xdr::ContractDataDurability::Persistent,
    });
    let (entry, _) = env.host().get_ledger_entry(&Rc::new(key)).unwrap()?;
    match &entry.data {
        soroban_sdk::xdr::LedgerEntryData::ContractData(d) => match &d.val {
            ScVal::ContractInstance(i) => Some(i.executable.clone()),
            other => panic!("the instance entry holds {other:?}"),
        },
        other => panic!("not contract data: {other:?}"),
    }
}

/// A fresh test host with a factory for `asset` and `second_asset`, the
/// fee address and one actor per role.
pub struct Fx {
    pub env: Env,
    pub factory: Address,
    pub wasm_hash: BytesN<32>,
    pub fee_address: Address,
    pub asset: Address,
    pub second_asset: Address,
    pub other_token: Address,
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
            li.sequence_number = 1_000;
            li.min_persistent_entry_ttl = 120_960;
            li.min_temp_entry_ttl = 720;
            li.max_entry_ttl = 3_110_400;
        });
        let asset = env
            .register_stellar_asset_contract_v2(account(&env, 97))
            .address();
        let second_asset = env
            .register_stellar_asset_contract_v2(account(&env, 98))
            .address();
        let other_token = env
            .register_stellar_asset_contract_v2(account(&env, 99))
            .address();
        let wasm_hash = env.deployer().upload_contract_wasm(mandate_wasm());
        let fee_address = account(&env, 7);
        let mut assets = Vec::new(&env);
        assets.push_back(asset.clone());
        assets.push_back(second_asset.clone());
        let factory = register_factory(&env, None, &wasm_hash, &assets, &fee_address).unwrap();
        Fx {
            factory,
            wasm_hash,
            fee_address,
            asset,
            second_asset,
            other_token,
            principal: account(&env, 1),
            agent: account(&env, 2),
            approver: account(&env, 3),
            guardian: account(&env, 4),
            executor: account(&env, 5),
            recovery: account(&env, 6),
            env,
        }
    }

    pub fn client(&self) -> UjiraFactoryClient<'_> {
        UjiraFactoryClient::new(&self.env, &self.factory)
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
            recipient_mode: MODE_ALLOWLIST,
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
            max_fee_bps: 25,
        }
    }

    pub fn predicted(&self, principal: &Address, salt: &BytesN<32>) -> Address {
        predicted_address(&self.env, &self.factory, principal, salt)
    }
}
