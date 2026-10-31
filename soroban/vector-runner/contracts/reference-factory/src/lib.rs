// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Test only, never deployed. A stand-in for the mandate factory that the
//! vector runner's self tests drive. It takes the constructor arguments and
//! answers `create` the way the runner expects the real factory to: the
//! principal authorizes, an unsupported asset is refused with the shared
//! code, the mandate lands at the address derived from the principal and the
//! salt, and `mandate_created` comes before the mandate's own
//! `mandate_initialized`.

#![no_std]

use mandate_errors::MandateError as Error;
use soroban_sdk::{
    contract, contractevent, contractimpl, contracttype, panic_with_error, xdr::ToXdr, Address,
    Bytes, BytesN, ContractExecutable, Env, Vec,
};

/// Same shape as the mandate's, repeated here so each stand-in builds to a
/// wasm of its own.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Limits {
    pub total_budget: i128,
    pub per_payment_cap: i128,
    pub per_recipient_daily_cap: i128,
    pub approval_threshold: i128,
    pub payout_ttl_seconds: u64,
    pub expires_at: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MandateConfig {
    pub principal: Address,
    pub asset: Address,
    pub recipient_mode: u32,
    pub tenant_ref: BytesN<32>,
    pub agents: Vec<Address>,
    pub approvers: Vec<Address>,
    pub approvals_required: u32,
    pub guardians: Vec<Address>,
    pub executors: Vec<Address>,
    pub recovery: Vec<Address>,
    pub recovery_threshold: u32,
    pub limits: Limits,
    pub attester: Option<Bytes>,
    pub country_policy_hash: BytesN<32>,
    pub max_fee_bps: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
enum Key {
    MandateWasm,
    Assets,
    FeeAddress,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MandateCreated {
    #[topic]
    pub tenant_ref: BytesN<32>,
    pub mandate: Address,
    pub principal: Address,
    pub asset: Address,
    pub spec_version: u32,
    pub fee_address: Address,
}

const SPEC_VERSION: u32 = 1;

#[contract]
pub struct ReferenceFactory;

#[contractimpl]
impl ReferenceFactory {
    pub fn __constructor(
        env: Env,
        mandate_wasm_hash: BytesN<32>,
        assets: Vec<Address>,
        fee_address: Address,
    ) {
        let store = env.storage().instance();
        store.set(&Key::MandateWasm, &mandate_wasm_hash);
        store.set(&Key::Assets, &assets);
        store.set(&Key::FeeAddress, &fee_address);
    }

    pub fn create(env: Env, config: MandateConfig, salt: BytesN<32>) -> Result<Address, Error> {
        config.principal.require_auth();
        let store = env.storage().instance();
        let assets: Vec<Address> = load(&env, &Key::Assets);
        if !assets.contains(&config.asset) {
            return Err(Error::AssetNotSupported);
        }
        let wasm: BytesN<32> = load(&env, &Key::MandateWasm);
        let fee_address: Address = load(&env, &Key::FeeAddress);
        // Two principals can pick the same salt, so the principal is part of
        // what the address is derived from.
        let mut seed: Bytes = config.principal.clone().to_xdr(&env);
        seed.append(&salt.into());
        let deployer = env
            .deployer()
            .with_current_contract(env.crypto().sha256(&seed).to_bytes());
        let mandate = deployer.deployed_address();
        // Emitted before the deploy, so it comes ahead of the mandate's own
        // creation event.
        MandateCreated {
            tenant_ref: config.tenant_ref.clone(),
            mandate: mandate.clone(),
            principal: config.principal.clone(),
            asset: config.asset.clone(),
            spec_version: SPEC_VERSION,
            fee_address: fee_address.clone(),
        }
        .publish(&env);
        let deployed = deployer.deploy_contract(
            ContractExecutable::Wasm(wasm),
            (env.current_contract_address(), fee_address, config),
        );
        if deployed != mandate {
            return Err(Error::InternalInvariant);
        }
        store.extend_ttl(17_280, 535_680);
        Ok(deployed)
    }
}

fn load<V: soroban_sdk::TryFromVal<Env, soroban_sdk::Val>>(env: &Env, key: &Key) -> V {
    match env.storage().instance().get(key) {
        Some(v) => v,
        None => panic_with_error!(env, Error::InternalInvariant),
    }
}
