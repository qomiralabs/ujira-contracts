// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Probe contracts for three host behaviors the mandate's rules lean on:
//!
//! - whether a failed token transfer can be caught and turned into our own
//!   error, and what happens to the caller's state when that error is returned;
//! - what a factory caller sees when the deployed contract's constructor fails;
//! - how a persistent entry behaves once its TTL has run out.
//!
//! None of this is mandate code. The probes are kept small so that the tests
//! assert host behavior and nothing else, and they run again on every SDK bump.

#![no_std]

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, xdr::ScErrorType, Address,
    BytesN, ContractExecutable, Env, Error, IntoVal, Symbol, Vec,
};

/// The mandate's transfer failure code, reused so the tests read like the
/// mandate's own error table.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ProbeError {
    ConfigInvalid = 34,
    TransferFailed = 72,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProbeKey {
    /// Written before a transfer is attempted, to see whether it survives.
    Marker,
    /// Stand-in for a payout record or tombstone.
    Record(u32),
    /// Same shape in temporary storage, for contrast.
    TempRecord(u32),
}

/// A caught failure as plain numbers: the host error type and its code.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Caught {
    pub error_type: u32,
    pub code: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Leg {
    pub to: Address,
    pub amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MarkerSet {
    pub amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordWritten {
    #[topic]
    pub id: u32,
    pub value: u32,
}

/// Every `ScErrorType`, so a caught error's type can be reported as a number
/// without depending on how the host packs it.
const ERROR_TYPES: [ScErrorType; 10] = [
    ScErrorType::Contract,
    ScErrorType::WasmVm,
    ScErrorType::Context,
    ScErrorType::Storage,
    ScErrorType::Object,
    ScErrorType::Crypto,
    ScErrorType::Events,
    ScErrorType::Budget,
    ScErrorType::Value,
    ScErrorType::Auth,
];

fn caught(err: Error) -> Caught {
    let mut error_type = u32::MAX;
    for t in ERROR_TYPES {
        if err.is_type(t) {
            error_type = t as u32;
        }
    }
    Caught {
        error_type,
        code: err.get_code(),
    }
}

/// Calls `token.transfer(this contract, to, amount)` without letting a token
/// failure abort the caller. A non-unit return from a token is treated as a
/// failure too, since the mandate cannot know what it means.
fn try_transfer(env: &Env, token: &Address, to: &Address, amount: i128) -> Result<(), Caught> {
    let args: Vec<soroban_sdk::Val> =
        (env.current_contract_address(), to.clone(), amount).into_val(env);
    match env.try_invoke_contract::<(), Error>(token, &Symbol::new(env, "transfer"), args) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err(Caught {
            error_type: ScErrorType::Value as u32,
            code: u32::MAX,
        }),
        Err(Ok(err)) => Err(caught(err)),
        // E is the host Error itself, so the conversion cannot fail; kept
        // explicit so the match stays total without an unwrap.
        Err(Err(_)) => Err(Caught {
            error_type: u32::MAX,
            code: u32::MAX,
        }),
    }
}

#[contract]
pub struct Payer;

#[contractimpl]
impl Payer {
    /// Mandate execute shape: change state first, then pay every leg, and map
    /// any token failure to our own error.
    pub fn pay(env: Env, token: Address, legs: Vec<Leg>) -> Result<(), ProbeError> {
        let mut total: i128 = 0;
        for leg in legs.iter() {
            total = total
                .checked_add(leg.amount)
                .ok_or(ProbeError::ConfigInvalid)?;
        }
        env.storage().persistent().set(&ProbeKey::Marker, &total);
        MarkerSet { amount: total }.publish(&env);
        for leg in legs.iter() {
            try_transfer(&env, &token, &leg.to, leg.amount)
                .map_err(|_| ProbeError::TransferFailed)?;
        }
        Ok(())
    }

    /// Revoke shape: change state, attempt the transfer, and succeed either
    /// way. Returns what the token reported if it failed.
    pub fn pay_or_keep(env: Env, token: Address, to: Address, amount: i128) -> Option<Caught> {
        env.storage().persistent().set(&ProbeKey::Marker, &amount);
        MarkerSet { amount }.publish(&env);
        try_transfer(&env, &token, &to, amount).err()
    }

    pub fn marker(env: Env) -> Option<i128> {
        env.storage().persistent().get(&ProbeKey::Marker)
    }
}

#[contract]
pub struct Factory;

#[contractimpl]
impl Factory {
    /// Deploys the child straight away and lets the constructor judge the
    /// arguments, which is the shape the mandate factory has today.
    pub fn create(
        env: Env,
        wasm_hash: BytesN<32>,
        salt: BytesN<32>,
        value: u32,
        fail: bool,
    ) -> Address {
        env.deployer()
            .with_current_contract(salt)
            .deploy_contract(ContractExecutable::Wasm(wasm_hash), (value, fail))
    }

    /// Checks the arguments in the factory before deploying, so the caller
    /// gets our error code instead of whatever the host makes of the
    /// constructor's failure.
    pub fn create_checked(
        env: Env,
        wasm_hash: BytesN<32>,
        salt: BytesN<32>,
        value: u32,
        fail: bool,
    ) -> Result<Address, ProbeError> {
        if fail {
            return Err(ProbeError::ConfigInvalid);
        }
        Ok(Self::create(env, wasm_hash, salt, value, fail))
    }

    pub fn predicted(env: Env, salt: BytesN<32>) -> Address {
        env.deployer()
            .with_current_contract(salt)
            .deployed_address()
    }
}

#[contract]
pub struct Store;

#[contractimpl]
impl Store {
    /// Writes a persistent record and pins its TTL to exactly `ttl` ledgers
    /// beyond this one, so a test knows when it lapses.
    pub fn put(env: Env, id: u32, value: u32, ttl: u32) {
        let key = ProbeKey::Record(id);
        env.storage().persistent().set(&key, &value);
        env.storage().persistent().extend_ttl(&key, ttl, ttl);
        RecordWritten { id, value }.publish(&env);
    }

    /// The uniqueness check the mandate makes before accepting a payout ID.
    pub fn has(env: Env, id: u32) -> bool {
        env.storage().persistent().has(&ProbeKey::Record(id))
    }

    pub fn get(env: Env, id: u32) -> Option<u32> {
        env.storage().persistent().get(&ProbeKey::Record(id))
    }

    pub fn put_temp(env: Env, id: u32, value: u32, ttl: u32) {
        let key = ProbeKey::TempRecord(id);
        env.storage().temporary().set(&key, &value);
        env.storage().temporary().extend_ttl(&key, ttl, ttl);
        RecordWritten { id, value }.publish(&env);
    }

    pub fn has_temp(env: Env, id: u32) -> bool {
        env.storage().temporary().has(&ProbeKey::TempRecord(id))
    }
}

#[cfg(test)]
mod test;
