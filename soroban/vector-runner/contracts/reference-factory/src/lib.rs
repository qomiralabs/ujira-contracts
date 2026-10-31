// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Test only, never deployed. A stand-in for the mandate factory that the
//! vector runner drives. It takes the constructor arguments and answers
//! `create` the way the runner expects the real factory to: the principal
//! authorizes, an unsupported asset is refused with the shared code, the
//! configuration is validated with the mandate's own shared validation before
//! anything is deployed, so a refused configuration returns the mandate's
//! code rather than a generic host error, the mandate lands at the address
//! derived from the principal and the salt, and `mandate_created` comes
//! before the mandate's own `mandate_initialized`.
//!
//! Until the production factory exists, CI also runs the shared creation
//! vectors through this stand-in against the real mandate wasm.

#![no_std]

use mandate_config::MandateConfig;
use mandate_errors::MandateError as Error;
use soroban_sdk::{
    contract, contractevent, contractimpl, contracttype, panic_with_error, xdr::ToXdr, Address,
    Bytes, BytesN, ContractExecutable, Env, Vec,
};

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
        // Validated before the deploy: a constructor error would reach the
        // caller only as a generic host error.
        mandate_config::validate(&env, &config, &fee_address)?;
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
