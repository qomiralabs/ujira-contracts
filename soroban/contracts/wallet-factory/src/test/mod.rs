// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Tests for the wallet factory. The factory is its compiled wasm, read from
//! the release folder that `scripts/soroban-build.sh` fills or from
//! `UJIRA_WALLET_FACTORY_WASM`, and a wasm older than its source is refused.
//! The smart account, its WebAuthn verifier and its weighted threshold policy
//! are the upstream wasm the CI job `smart-account` built from the pinned tag,
//! committed under `fixtures/`; a test checks each one against the hash the
//! build script pins.

extern crate std;

mod wallet_factory;

use crate::{Signer, UjiraWalletFactoryClient, WalletFactoryError};
use soroban_sdk::{
    contracttype,
    testutils::EnvTestConfig,
    xdr::{
        ContractEventBody, ContractEventType, ContractExecutable, ContractId, ContractIdPreimage,
        ContractIdPreimageFromAddress, Hash, HashIdPreimage, HashIdPreimageContractId, LedgerKey,
        LedgerKeyContractData, Limited, Limits, ReadXdr, ScAddress, ScError, ScSpecEntry, ScVal,
        Uint256, WriteXdr,
    },
    Address, Bytes, BytesN, Env, IntoVal, Map, String, TryFromVal, Val, Vec,
};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::string::ToString;
use std::sync::OnceLock;
use std::time::SystemTime;

pub const ACCOUNT_WASM: &[u8] = include_bytes!("../../fixtures/multisig_account_example.wasm");
pub const VERIFIER_WASM: &[u8] =
    include_bytes!("../../fixtures/multisig_webauthn_verifier_example.wasm");
pub const POLICY_WASM: &[u8] =
    include_bytes!("../../fixtures/multisig_weighted_threshold_policy_example.wasm");

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

pub fn factory_wasm() -> &'static [u8] {
    static WASM: OnceLock<std::vec::Vec<u8>> = OnceLock::new();
    WASM.get_or_init(|| {
        let var = "UJIRA_WALLET_FACTORY_WASM";
        let path = match std::env::var_os(var) {
            Some(p) => PathBuf::from(p),
            None => manifest().join("../../target/wasm32v1-none/release/ujira_wallet_factory.wasm"),
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
        newest_source(
            &manifest().join("src"),
            &manifest().join("src/test"),
            &mut newest,
        );
        assert!(
            std::env::var_os(var).is_some() || built >= newest,
            "{} is older than the contract source; rebuild it with scripts/soroban-build.sh",
            path.display()
        );
        std::fs::read(&path).unwrap()
    })
}

/// The hash `scripts/build-smart-account.sh` pins for one contract, read from
/// the script itself so the two can never drift apart.
pub fn pin(name: &str) -> std::string::String {
    let script =
        std::fs::read_to_string(manifest().join("../../../scripts/build-smart-account.sh"))
            .unwrap();
    let prefix = std::format!("pin_{name}=");
    let line = script
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no {prefix} line in the build script"));
    line[prefix.len()..].to_string()
}

/// The bytes of a host value, for building XDR by hand.
pub fn raw(b: &Bytes) -> std::vec::Vec<u8> {
    b.iter().collect()
}

pub fn hex(bytes: &[u8]) -> std::string::String {
    bytes.iter().map(|b| std::format!("{b:02x}")).collect()
}

pub fn sha256(env: &Env, data: &[u8]) -> [u8; 32] {
    env.crypto()
        .sha256(&Bytes::from_slice(env, data))
        .to_array()
}

/// The install parameters of the weighted threshold policy, with the field
/// names and types of the upstream struct, so they encode the same.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WeightedThresholdAccountParams {
    pub signer_weights: Map<Signer, u32>,
    pub threshold: u32,
}

/// The upstream rule types and rule, as the account returns them.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContextRuleType {
    Default,
    CallContract(Address),
    CreateContract(BytesN<32>),
    CreateContractExternalRef(Address, String),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextRule {
    pub id: u32,
    pub context_type: ContextRuleType,
    pub name: String,
    pub signers: Vec<Signer>,
    pub signer_ids: Vec<u32>,
    pub policies: Vec<Address>,
    pub policy_ids: Vec<u32>,
    pub valid_until: Option<u32>,
}

/// A host with the account code uploaded and one verifier and one threshold
/// policy deployed, as they would be once per network.
pub struct Fx {
    pub env: Env,
    pub account_hash: BytesN<32>,
    pub verifier: Address,
    pub threshold: Address,
}

impl Fx {
    pub fn new() -> Fx {
        let env = Env::new_with_config(EnvTestConfig {
            capture_snapshot_at_drop: false,
        });
        let account_hash = env.deployer().upload_contract_wasm(ACCOUNT_WASM);
        let verifier = env.register(VERIFIER_WASM, ());
        let threshold = env.register(POLICY_WASM, ());
        Fx {
            env,
            account_hash,
            verifier,
            threshold,
        }
    }

    pub fn factory(&self) -> UjiraWalletFactoryClient<'_> {
        self.factory_with(&Vec::new(&self.env))
    }

    pub fn factory_with(&self, deny: &Vec<Address>) -> UjiraWalletFactoryClient<'_> {
        self.try_factory_with(deny).unwrap()
    }

    /// Registers the compiled factory, or returns the constructor's error.
    pub fn try_factory_with(
        &self,
        deny: &Vec<Address>,
    ) -> Result<UjiraWalletFactoryClient<'_>, WalletFactoryError> {
        let args = (self.account_hash.clone(), deny.clone());
        let env = &self.env;
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            env.register(factory_wasm(), args)
        })) {
            Ok(address) => Ok(UjiraWalletFactoryClient::new(env, &address)),
            Err(_) => Err(last_contract_error(env)
                .and_then(|code| {
                    WalletFactoryError::try_from(soroban_sdk::Error::from_contract_error(code)).ok()
                })
                .expect("the constructor failed without a factory error")),
        }
    }

    /// A passkey signer under this host's verifier. The key is the 65-byte
    /// uncompressed form; the verifier only checks it when a signature is
    /// verified, which nothing here does.
    pub fn passkey(&self, n: u8) -> Signer {
        Signer::External(self.verifier.clone(), passkey_key(&self.env, n))
    }

    pub fn salt(&self, n: u8) -> BytesN<32> {
        BytesN::from_array(&self.env, &[n; 32])
    }

    /// The weighted threshold policy with weight 1 per signer.
    pub fn threshold_policy(&self, signers: &Vec<Signer>, threshold: u32) -> Map<Address, Val> {
        let mut weights = Map::new(&self.env);
        for signer in signers.iter() {
            weights.set(signer, 1u32);
        }
        let params = WeightedThresholdAccountParams {
            signer_weights: weights,
            threshold,
        };
        let mut policies = Map::new(&self.env);
        policies.set(self.threshold.clone(), params.into_val(&self.env));
        policies
    }
}

pub fn passkey_key(env: &Env, n: u8) -> Bytes {
    let mut key = [n; 65];
    key[0] = 0x04;
    Bytes::from_array(env, &key)
}

/// The same 65-byte key followed by a credential ID, which the verifier
/// canonicalizes back to the bare key.
pub fn passkey_key_with_credential(env: &Env, n: u8) -> Bytes {
    let mut key = passkey_key(env, n);
    key.extend_from_array(&[0xcc; 16]);
    key
}

/// The mandate factory's error helpers have the same need: a failed
/// constructor's code shows up only in the diagnostic events.
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

/// The mandate error a refused call returned. Anything else fails the test.
pub fn code<T: core::fmt::Debug, C: core::fmt::Debug, I: core::fmt::Debug>(
    r: Result<Result<T, C>, Result<WalletFactoryError, I>>,
) -> WalletFactoryError {
    match r {
        Err(Ok(e)) => e,
        other => panic!("expected a factory error, got {other:?}"),
    }
}

pub fn account(env: &Env, seed: u8) -> Address {
    Address::try_from_val(
        env,
        &ScAddress::Account(soroban_sdk::xdr::AccountId(
            soroban_sdk::xdr::PublicKey::PublicKeyTypeEd25519(Uint256([seed; 32])),
        )),
    )
    .unwrap()
}

pub fn contract(env: &Env, seed: u8) -> Address {
    Address::try_from_val(env, &ScAddress::Contract(ContractId(Hash([seed; 32])))).unwrap()
}

/// The code hash a deployed contract runs.
pub fn executable_of(env: &Env, contract: &Address) -> BytesN<32> {
    let key = LedgerKey::ContractData(LedgerKeyContractData {
        contract: ScAddress::from(contract),
        key: ScVal::LedgerKeyContractInstance,
        durability: soroban_sdk::xdr::ContractDataDurability::Persistent,
    });
    let (entry, _) = env
        .host()
        .get_ledger_entry(&Rc::new(key))
        .unwrap()
        .expect("no contract at that address");
    match &entry.data {
        soroban_sdk::xdr::LedgerEntryData::ContractData(d) => match &d.val {
            ScVal::ContractInstance(i) => match &i.executable {
                ContractExecutable::Wasm(Hash(h)) => BytesN::from_array(env, h),
                other => panic!("not a wasm contract: {other:?}"),
            },
            other => panic!("the instance entry holds {other:?}"),
        },
        other => panic!("not contract data: {other:?}"),
    }
}

/// Where `deploy` puts a wallet, computed from XDR as a client would, without
/// asking any contract: the signers in the order the host sorts them, which
/// for passkeys under one verifier is the order of their key bytes, then the
/// policies and the salt.
pub fn offchain_address(
    env: &Env,
    factory: &Address,
    signers: &[(ScAddress, std::vec::Vec<u8>)],
    salt: &BytesN<32>,
) -> Address {
    let mut sorted = signers.to_vec();
    sorted.sort_by(|a, b| a.1.cmp(&b.1));
    let signer_vals: std::vec::Vec<ScVal> = sorted
        .into_iter()
        .map(|(verifier, key)| {
            ScVal::Vec(Some(
                std::vec![
                    ScVal::Symbol("External".try_into().unwrap()),
                    ScVal::Address(verifier),
                    ScVal::Bytes(key.try_into().unwrap()),
                ]
                .try_into()
                .unwrap(),
            ))
        })
        .collect();
    let mut seed = ScVal::Vec(Some(signer_vals.try_into().unwrap()))
        .to_xdr(Limits::none())
        .unwrap();
    seed.extend(
        ScVal::Map(Some(std::vec::Vec::new().try_into().unwrap()))
            .to_xdr(Limits::none())
            .unwrap(),
    );
    seed.extend_from_slice(&salt.to_array());
    let preimage = HashIdPreimage::ContractId(HashIdPreimageContractId {
        network_id: Hash(env.ledger().network_id().to_array()),
        contract_id_preimage: ContractIdPreimage::Address(ContractIdPreimageFromAddress {
            address: ScAddress::from(factory),
            salt: Uint256(sha256(env, &seed)),
        }),
    });
    let id = sha256(env, &preimage.to_xdr(Limits::none()).unwrap());
    Address::try_from_val(env, &ScAddress::Contract(ContractId(Hash(id)))).unwrap()
}

fn leb128(b: &[u8], i: &mut usize) -> usize {
    let mut value = 0usize;
    let mut shift = 0;
    loop {
        let byte = b[*i];
        *i += 1;
        value |= usize::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
        shift += 7;
    }
}

/// The payload of the wasm's `contractspecv0` custom section, which is what
/// a network and every client read as the contract's interface.
fn spec_section(wasm: &[u8]) -> &[u8] {
    let mut i = 8;
    while i < wasm.len() {
        let id = wasm[i];
        i += 1;
        let size = leb128(wasm, &mut i);
        let end = i + size;
        if id == 0 {
            let mut j = i;
            let n = leb128(wasm, &mut j);
            if &wasm[j..j + n] == b"contractspecv0" {
                return &wasm[j + n..end];
            }
        }
        i = end;
    }
    panic!("the wasm has no contract spec");
}

/// Every function the factory's wasm declares, with its input names.
pub fn functions() -> std::vec::Vec<(std::string::String, std::vec::Vec<std::string::String>)> {
    let mut r = Limited::new(
        std::io::Cursor::new(spec_section(factory_wasm())),
        Limits::none(),
    );
    let mut out = std::vec::Vec::new();
    for entry in ScSpecEntry::read_xdr_iter(&mut r) {
        if let ScSpecEntry::FunctionV0(f) = entry.unwrap() {
            let inputs = f
                .inputs
                .iter()
                .map(|i| i.name.to_utf8_string_lossy())
                .collect();
            out.push((f.name.to_utf8_string_lossy(), inputs));
        }
    }
    out
}

pub fn inputs_of(name: &str) -> std::vec::Vec<std::string::String> {
    functions()
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("the factory declares no {name}"))
        .1
}

/// The account's own view of its rules.
pub fn rules_count(env: &Env, wallet: &Address) -> u32 {
    env.invoke_contract(
        wallet,
        &soroban_sdk::Symbol::new(env, "get_context_rules_count"),
        Vec::new(env),
    )
}

pub fn rule(env: &Env, wallet: &Address, id: u32) -> ContextRule {
    env.invoke_contract(
        wallet,
        &soroban_sdk::Symbol::new(env, "get_context_rule"),
        Vec::from_array(env, [id.into_val(env)]),
    )
}

pub fn threshold_of(env: &Env, policy: &Address, rule_id: u32, wallet: &Address) -> u32 {
    env.invoke_contract(
        policy,
        &soroban_sdk::Symbol::new(env, "get_threshold"),
        Vec::from_array(env, [rule_id.into_val(env), wallet.into_val(env)]),
    )
}
