// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The passkey wallet factory.
//!
//! A principal's wallet must run exactly the audited OpenZeppelin smart
//! account and nothing else, and UJIRA must never be able to sign for it or
//! change it. This factory makes both checkable by anyone. It deploys one wasm
//! hash, fixed at construction, and there is no parameter that names another.
//! It has no admin, no setter, no upgrade and no fee, so what it was built with
//! holds for as long as it exists; a different hash or deny list means a new
//! factory, at a new address.
//!
//! The wallet's address is the contract ID the host derives from this factory's
//! address and the deployer salt
//! `SHA-256(signers_xdr || policies_xdr || salt)`, so it commits to the wallet's
//! whole starting configuration. Nobody can take a predicted address with
//! different signers, which is why anyone may call `deploy` and it needs no
//! authorization: a sponsor can pay for it without being able to change it.
//!
//! The account's constructor makes one rule from these arguments, a `Default`
//! rule that covers every call and so acts as the wallet's admin rule. A rule
//! for one mandate cannot be set here, because the mandate's address derives
//! from the wallet's, and the wallet's would then have to hash its own
//! mandate's address. The wallet's signers add that rule themselves afterwards.

#![no_std]

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, xdr::ToXdr, Address, Bytes,
    BytesN, ContractExecutable, Env, Map, TryFromVal, Val, Vec,
};

#[cfg(test)]
mod test;

/// One ledger every 5 seconds.
const DAY_IN_LEDGERS: u32 = 17_280;
/// Every call keeps the factory alive for at least 30 more days, topping up to
/// 31, so a factory in use never archives.
const INSTANCE_TTL_EXTEND_TO: u32 = 31 * DAY_IN_LEDGERS;
const INSTANCE_TTL_THRESHOLD: u32 = INSTANCE_TTL_EXTEND_TO - DAY_IN_LEDGERS;

/// The XDR of an account address is a 12 byte header and its 32 byte key.
const ACCOUNT_XDR_BYTES: u32 = 44;
const ACCOUNT_KEY_OFFSET: u32 = 12;

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum WalletFactoryError {
    /// A value the constructor always writes is missing.
    NotInitialized = 1,
    /// A wallet with no signers could never authorize anything.
    NoSigners = 2,
    /// The same signer was named twice.
    RepeatedSigner = 3,
    /// A signer is this factory or on the deny list.
    SignerDenied = 4,
    /// A policy is this factory or on the deny list.
    PolicyDenied = 5,
    /// The deny list names an address twice.
    DenyListInvalid = 6,
    /// The host deployed somewhere other than the predicted address.
    AddressMismatch = 7,
}

/// A signer, exactly as the smart account's `stellar-accounts` crate defines it,
/// so the two encode to the same bytes. It is written out here rather than
/// imported because that crate is built against an older soroban-sdk than this
/// workspace, and two SDK versions cannot share a type.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Signer {
    /// Another address, which authorizes through its own `require_auth`.
    Delegated(Address),
    /// A verifier contract and the public key it checks.
    External(Address, Bytes),
}

/// Set once by the constructor. Nothing writes these again.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
enum Key {
    AccountWasm,
    DenyList,
}

/// A wallet was deployed. The address commits to its starting signers and
/// policies, and the code hash says what it runs.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WalletDeployed {
    #[topic]
    pub wallet: Address,
    pub wasm_hash: BytesN<32>,
    pub salt: BytesN<32>,
}

#[contract]
pub struct UjiraWalletFactory;

#[contractimpl]
impl UjiraWalletFactory {
    /// Fixes the account code and the deny list for the life of the factory.
    /// The deny list names what UJIRA itself holds, so that no wallet can be
    /// born with a UJIRA key or contract able to act for it.
    pub fn __constructor(
        env: Env,
        account_wasm_hash: BytesN<32>,
        deny_list: Vec<Address>,
    ) -> Result<(), WalletFactoryError> {
        if has_repeat(&deny_list) {
            return Err(WalletFactoryError::DenyListInvalid);
        }
        let store = env.storage().instance();
        store.set(&Key::AccountWasm, &account_wasm_hash);
        store.set(&Key::DenyList, &deny_list);
        extend_instance(&env);
        Ok(())
    }

    /// Deploys a wallet and returns its address, which is what
    /// `predict_address` returns for the same arguments. The signers may come
    /// in any order; they are sorted first, so one set has one address.
    pub fn deploy(
        env: Env,
        signers: Vec<Signer>,
        policies: Map<Address, Val>,
        salt: BytesN<32>,
    ) -> Result<Address, WalletFactoryError> {
        let signers = check(&env, &signers, &policies)?;
        let wasm_hash = account_wasm_hash(&env)?;
        let deployer = env
            .deployer()
            .with_current_contract(deployer_salt(&env, &signers, &policies, &salt));
        let predicted = deployer.deployed_address();
        let wallet = deployer.deploy_contract(
            ContractExecutable::Wasm(wasm_hash.clone()),
            (signers, policies),
        );
        // The event names this address, so it must be the deployed one.
        if wallet != predicted {
            return Err(WalletFactoryError::AddressMismatch);
        }
        WalletDeployed {
            wallet: wallet.clone(),
            wasm_hash,
            salt,
        }
        .publish(&env);
        extend_instance(&env);
        Ok(wallet)
    }

    /// The address `deploy` gives these arguments, whether or not it has run.
    /// It applies the same refusals, so an address it returns is one `deploy`
    /// can reach.
    pub fn predict_address(
        env: Env,
        signers: Vec<Signer>,
        policies: Map<Address, Val>,
        salt: BytesN<32>,
    ) -> Result<Address, WalletFactoryError> {
        let signers = check(&env, &signers, &policies)?;
        extend_instance(&env);
        Ok(env
            .deployer()
            .with_current_contract(deployer_salt(&env, &signers, &policies, &salt))
            .deployed_address())
    }

    /// The one code hash every wallet from this factory starts with.
    pub fn account_wasm_hash(env: Env) -> Result<BytesN<32>, WalletFactoryError> {
        extend_instance(&env);
        account_wasm_hash(&env)
    }

    /// What this factory refuses as a signer or a policy, besides itself.
    pub fn deny_list(env: Env) -> Result<Vec<Address>, WalletFactoryError> {
        extend_instance(&env);
        deny_list(&env)
    }
}

/// The refusals, and the signers in their one sorted order. The account's own
/// constructor repeats its checks on what arrives, including duplicates under
/// a different key encoding, which only the verifier can see.
fn check(
    env: &Env,
    signers: &Vec<Signer>,
    policies: &Map<Address, Val>,
) -> Result<Vec<Signer>, WalletFactoryError> {
    if signers.is_empty() {
        return Err(WalletFactoryError::NoSigners);
    }
    let denied = deny_list(env)?;
    let factory = env.current_contract_address();
    let mut sorted: Map<Signer, ()> = Map::new(env);
    for signer in signers.iter() {
        if is_denied_signer(env, &signer, &denied, &factory) {
            return Err(WalletFactoryError::SignerDenied);
        }
        sorted.set(signer, ());
    }
    if sorted.len() != signers.len() {
        return Err(WalletFactoryError::RepeatedSigner);
    }
    for policy in policies.keys().iter() {
        if policy == factory || denied.contains(&policy) {
            return Err(WalletFactoryError::PolicyDenied);
        }
    }
    Ok(sorted.keys())
}

/// A denied address may appear as a delegate, as a verifier, or, if it is an
/// account, as the raw key of an external signer. The last is what stops a
/// UJIRA account's key being listed under a verifier instead of by address; the
/// ed25519 verifier takes exactly the 32 byte key, so equality is the test.
fn is_denied_signer(env: &Env, signer: &Signer, denied: &Vec<Address>, factory: &Address) -> bool {
    match signer {
        Signer::Delegated(address) => address == factory || denied.contains(address),
        Signer::External(verifier, key) => {
            verifier == factory
                || denied.contains(verifier)
                || denied
                    .iter()
                    .any(|d| account_key(env, &d).as_ref() == Some(key))
        }
    }
}

/// The ed25519 key behind an account address, or nothing for a contract.
fn account_key(env: &Env, address: &Address) -> Option<Bytes> {
    let xdr = address.clone().to_xdr(env);
    (xdr.len() == ACCOUNT_XDR_BYTES).then(|| xdr.slice(ACCOUNT_KEY_OFFSET..))
}

/// `SHA-256(signers_xdr || policies_xdr || salt)`. The signers are the sorted
/// list, and a host map is always in key order, so both encode one way.
fn deployer_salt(
    env: &Env,
    signers: &Vec<Signer>,
    policies: &Map<Address, Val>,
    salt: &BytesN<32>,
) -> BytesN<32> {
    let mut seed: Bytes = signers.clone().to_xdr(env);
    seed.append(&policies.clone().to_xdr(env));
    seed.append(&Bytes::from(salt.clone()));
    env.crypto().sha256(&seed).to_bytes()
}

fn account_wasm_hash(env: &Env) -> Result<BytesN<32>, WalletFactoryError> {
    load(env, &Key::AccountWasm)
}

fn deny_list(env: &Env) -> Result<Vec<Address>, WalletFactoryError> {
    load(env, &Key::DenyList)
}

fn load<V: TryFromVal<Env, Val>>(env: &Env, key: &Key) -> Result<V, WalletFactoryError> {
    env.storage()
        .instance()
        .get(key)
        .ok_or(WalletFactoryError::NotInitialized)
}

fn extend_instance(env: &Env) {
    let to = INSTANCE_TTL_EXTEND_TO.min(env.storage().max_ttl());
    let threshold = INSTANCE_TTL_THRESHOLD.min(to);
    env.storage().instance().extend_ttl(threshold, to);
}

fn has_repeat(list: &Vec<Address>) -> bool {
    list.iter()
        .enumerate()
        .any(|(i, a)| list.iter().skip(i + 1).any(|b| a == b))
}
