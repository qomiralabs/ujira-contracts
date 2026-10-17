// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! A contract whose constructor fails on request. The factory probe deploys it
//! to see what a failed constructor looks like from the outside, so it writes
//! state and emits an event before failing: both must be gone afterwards.
//!
//! It also doubles as a hostile token: its `transfer` never returns, so a
//! caller can see what happens when a token burns the whole budget.

#![no_std]

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, Address, Env, Vec,
};

/// Same number the mandate uses for a bad config, so a test can tell whether
/// the code survived the trip through the deployer.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ChildError {
    ConfigInvalid = 34,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChildKey {
    Value,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChildInitialized {
    pub value: u32,
}

#[contract]
pub struct Child;

#[contractimpl]
impl Child {
    pub fn __constructor(env: Env, value: u32, fail: bool) -> Result<(), ChildError> {
        env.storage().instance().set(&ChildKey::Value, &value);
        ChildInitialized { value }.publish(&env);
        if fail {
            return Err(ChildError::ConfigInvalid);
        }
        Ok(())
    }

    pub fn value(env: Env) -> Option<u32> {
        env.storage().instance().get(&ChildKey::Value)
    }

    /// Token-shaped on purpose. Grows a vector until the host stops it.
    pub fn transfer(env: Env, _from: Address, _to: Address, _amount: i128) {
        let mut v: Vec<u32> = Vec::new(&env);
        loop {
            v.push_back(1);
        }
    }
}
