// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The UJIRA mandate factory.
//!
//! Every mandate is its own contract, and its address is where the principal
//! funds it. A mandate cannot prove who deployed it, so what makes it
//! trustworthy is this factory: the mandate's address derives from the
//! factory and the principal's salt, and the factory announces it with
//! `mandate_created`. Off-chain code that recomputes the address from a
//! factory it trusts, or reads that factory's event, knows the mandate runs
//! the code and the fee address the factory was built with.
//!
//! For that to hold, everything the factory was built with is fixed forever:
//! the mandate's code hash, the supported assets and the fee address. There
//! is no admin, no setter and no upgrade; a different list or fee address
//! means a new factory for new mandates.
//!
//! The mandate address is the contract ID the host derives from this
//! factory's address and the deployer salt `SHA-256(principal_xdr || salt)`,
//! where `principal_xdr` is the principal as an XDR `ScVal` address. Two
//! principals choosing the same salt get different addresses, and nobody
//! but the principal can take a principal's address, because `create`
//! needs the principal's authorization.

#![no_std]

use mandate_config::{MandateConfig, SPEC_VERSION};
use mandate_errors::MandateError;
use soroban_sdk::{
    contract, contractevent, contractimpl, contracttype, xdr::ToXdr, Address, Bytes, BytesN,
    ContractExecutable, Env, TryFromVal, Val, Vec,
};

/// One ledger every 5 seconds.
const DAY_IN_LEDGERS: u32 = 17_280;
/// Every call keeps the factory and its code alive for at least 30 more
/// days, topping up to 31, so a factory in use never archives.
const INSTANCE_TTL_EXTEND_TO: u32 = 31 * DAY_IN_LEDGERS;
const INSTANCE_TTL_THRESHOLD: u32 = INSTANCE_TTL_EXTEND_TO - DAY_IN_LEDGERS;

/// Set once by the constructor. Nothing writes these again.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
enum Key {
    MandateWasm,
    Assets,
    FeeAddress,
}

/// Announces a new mandate. The topic key is the tenant tag, as for
/// every mandate event that is not about a payout.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MandateCreated {
    #[topic]
    pub tenant_tag: BytesN<32>,
    pub mandate: Address,
    pub principal: Address,
    pub asset: Address,
    pub spec_version: u32,
    pub fee_address: Address,
}

#[contract]
pub struct UjiraFactory;

#[contractimpl]
impl UjiraFactory {
    /// Fixes the mandate code, the supported assets and the fee address for
    /// the life of the factory.
    ///
    /// Each of these can only be wrong once, so a mistake is refused here
    /// rather than discovered later: an empty or repeated asset list, and a
    /// fee address that can never be spent from. Stellar has no zero
    /// address, but the account and the contract with an all-zero key are
    /// valid addresses nobody controls, and fees sent to a token contract or
    /// to this factory would be stuck there for good.
    pub fn __constructor(
        env: Env,
        mandate_wasm_hash: BytesN<32>,
        assets: Vec<Address>,
        fee_address: Address,
    ) -> Result<(), MandateError> {
        if assets.is_empty() || has_repeat(&assets) {
            return Err(MandateError::ConfigInvalid);
        }
        if is_zero_address(&env, &fee_address)
            || assets.contains(&fee_address)
            || fee_address == env.current_contract_address()
        {
            return Err(MandateError::ConfigInvalid);
        }
        let store = env.storage().instance();
        store.set(&Key::MandateWasm, &mandate_wasm_hash);
        store.set(&Key::Assets, &assets);
        store.set(&Key::FeeAddress, &fee_address);
        extend_instance(&env);
        Ok(())
    }

    /// Deploys a mandate for `config.principal`, which must authorize the
    /// call, and returns its address.
    ///
    /// The checks run in a fixed order: the principal's authorization, the
    /// asset, then the full configuration with the mandate's own shared
    /// validation. The configuration is validated here, before the deploy,
    /// because a constructor error reaches the caller only as a generic host
    /// error; the mandate's constructor runs the same validation again as a
    /// backstop. A refused create leaves no contract and no events behind, so
    /// the same salt can be used again.
    pub fn create(
        env: Env,
        config: MandateConfig,
        salt: BytesN<32>,
    ) -> Result<Address, MandateError> {
        config.principal.require_auth();
        if !assets(&env)?.contains(&config.asset) {
            return Err(MandateError::AssetNotSupported);
        }
        let fee_address = fee_address(&env)?;
        mandate_config::validate(&env, &config, &fee_address)?;
        let wasm = mandate_wasm_hash(&env)?;

        let deployer =
            env.deployer()
                .with_current_contract(deployer_salt(&env, &config.principal, &salt));
        let mandate = deployer.deployed_address();

        // Published before the deploy so it comes ahead of the mandate's own
        // mandate_initialized. If the deploy fails the whole call fails, and
        // the event goes with it.
        MandateCreated {
            tenant_tag: config.tenant_tag.clone(),
            mandate: mandate.clone(),
            principal: config.principal.clone(),
            asset: config.asset.clone(),
            spec_version: SPEC_VERSION,
            fee_address: fee_address.clone(),
        }
        .publish(&env);

        // The fee address always comes from here, never from the principal.
        let deployed = deployer.deploy_contract(
            ContractExecutable::Wasm(wasm),
            (env.current_contract_address(), fee_address, config),
        );
        // The announced address must be the deployed one, or the event would
        // vouch for a contract that is not there.
        if deployed != mandate {
            return Err(MandateError::InternalInvariant);
        }
        extend_instance(&env);
        Ok(deployed)
    }

    /// The assets a mandate may settle in, in the order they were given.
    pub fn supported_assets(env: Env) -> Result<Vec<Address>, MandateError> {
        extend_instance(&env);
        assets(&env)
    }

    /// The hash of the mandate code every mandate from this factory runs.
    pub fn mandate_wasm_hash(env: Env) -> Result<BytesN<32>, MandateError> {
        extend_instance(&env);
        mandate_wasm_hash(&env)
    }

    /// Where every mandate from this factory pays its fees.
    pub fn fee_address(env: Env) -> Result<Address, MandateError> {
        extend_instance(&env);
        fee_address(&env)
    }

    /// The spec version, the same number `mandate_created` carries.
    pub fn version(env: Env) -> u32 {
        extend_instance(&env);
        SPEC_VERSION
    }
}

/// The salt handed to the deployer. The principal is part of it so that one
/// principal's salt can never collide with another's.
fn deployer_salt(env: &Env, principal: &Address, salt: &BytesN<32>) -> BytesN<32> {
    let mut seed: Bytes = principal.clone().to_xdr(env);
    seed.append(&Bytes::from(salt.clone()));
    env.crypto().sha256(&seed).to_bytes()
}

fn assets(env: &Env) -> Result<Vec<Address>, MandateError> {
    load(env, &Key::Assets)
}

fn fee_address(env: &Env) -> Result<Address, MandateError> {
    load(env, &Key::FeeAddress)
}

fn mandate_wasm_hash(env: &Env) -> Result<BytesN<32>, MandateError> {
    load(env, &Key::MandateWasm)
}

/// The constructor always writes every key, so a missing one is an
/// invariant failure, returned rather than trapped so the cause stays
/// readable.
fn load<V: TryFromVal<Env, Val>>(env: &Env, key: &Key) -> Result<V, MandateError> {
    env.storage()
        .instance()
        .get(key)
        .ok_or(MandateError::InternalInvariant)
}

fn extend_instance(env: &Env) {
    let to = INSTANCE_TTL_EXTEND_TO.min(env.storage().max_ttl());
    let threshold = INSTANCE_TTL_THRESHOLD.min(to);
    env.storage().instance().extend_ttl(threshold, to);
}

/// An account or contract address whose 32-byte key or hash is all zeros.
/// Both encode as an XDR `ScVal` address that ends with those 32 bytes.
fn is_zero_address(env: &Env, address: &Address) -> bool {
    let xdr = address.clone().to_xdr(env);
    match xdr.len().checked_sub(32) {
        Some(start) => xdr.slice(start..).iter().all(|b| b == 0),
        None => false,
    }
}

fn has_repeat(list: &Vec<Address>) -> bool {
    list.iter()
        .enumerate()
        .any(|(i, item)| list.first_index_of(&item) != u32::try_from(i).ok())
}

#[cfg(test)]
mod test;
