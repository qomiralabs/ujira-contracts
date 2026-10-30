// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Test only, never deployed. The `other_token` kinds of the shared vectors
//! that a Stellar Asset Contract cannot be:
//!
//! - kind 1, moves_settlement: its transfer moves its own balance and then
//!   tries to move the same amount of the settlement asset from the same
//!   holder, the way a second address for the same asset would. On Stellar
//!   the holder authorized only this token's transfer, so the nested call
//!   fails and takes the whole transfer with it.
//! - kind 2, returns_value: its transfer moves the tokens and then returns
//!   `true`, where a SEP-41 transfer returns nothing.
//!
//! `mint` needs no authorization because only the runner's setup calls it.

#![no_std]

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, panic_with_error, vec,
    Address, Env, IntoVal, MuxedAddress, Symbol, Val,
};

pub const KIND_MOVES_SETTLEMENT: u32 = 1;
pub const KIND_RETURNS_VALUE: u32 = 2;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    InsufficientBalance = 1,
    NegativeAmount = 2,
    UnknownKind = 3,
    Overflow = 4,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
enum Key {
    Kind,
    Settlement,
    Balance(Address),
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Transfer {
    #[topic]
    pub from: Address,
    #[topic]
    pub to: Address,
    pub amount: i128,
}

#[contract]
pub struct TestToken;

#[contractimpl]
impl TestToken {
    pub fn __constructor(env: Env, kind: u32, settlement: Address) {
        if kind != KIND_MOVES_SETTLEMENT && kind != KIND_RETURNS_VALUE {
            panic_with_error!(&env, Error::UnknownKind);
        }
        let store = env.storage().instance();
        store.set(&Key::Kind, &kind);
        store.set(&Key::Settlement, &settlement);
    }

    pub fn decimals() -> u32 {
        7
    }

    pub fn balance(env: Env, id: Address) -> i128 {
        read_balance(&env, &id)
    }

    pub fn mint(env: Env, to: Address, amount: i128) {
        if amount < 0 {
            panic_with_error!(&env, Error::NegativeAmount);
        }
        let next = match read_balance(&env, &to).checked_add(amount) {
            Some(v) => v,
            None => panic_with_error!(&env, Error::Overflow),
        };
        write_balance(&env, &to, next);
    }

    pub fn transfer(env: Env, from: Address, to: MuxedAddress, amount: i128) -> Val {
        from.require_auth();
        if amount < 0 {
            panic_with_error!(&env, Error::NegativeAmount);
        }
        let to_address = to.address();
        let have = read_balance(&env, &from);
        if have < amount {
            panic_with_error!(&env, Error::InsufficientBalance);
        }
        write_balance(&env, &from, have - amount);
        let next = match read_balance(&env, &to_address).checked_add(amount) {
            Some(v) => v,
            None => panic_with_error!(&env, Error::Overflow),
        };
        write_balance(&env, &to_address, next);
        Transfer {
            from: from.clone(),
            to: to_address,
            amount,
        }
        .publish(&env);

        let kind: u32 = env.storage().instance().get(&Key::Kind).unwrap_or(0);
        if kind == KIND_MOVES_SETTLEMENT {
            let settlement: Address = match env.storage().instance().get(&Key::Settlement) {
                Some(a) => a,
                None => panic_with_error!(&env, Error::UnknownKind),
            };
            env.invoke_contract::<()>(
                &settlement,
                &Symbol::new(&env, "transfer"),
                vec![
                    &env,
                    from.into_val(&env),
                    to.into_val(&env),
                    amount.into_val(&env),
                ],
            );
            return ().into_val(&env);
        }
        true.into_val(&env)
    }
}

fn read_balance(env: &Env, id: &Address) -> i128 {
    env.storage()
        .persistent()
        .get(&Key::Balance(id.clone()))
        .unwrap_or(0)
}

fn write_balance(env: &Env, id: &Address, amount: i128) {
    env.storage()
        .persistent()
        .set(&Key::Balance(id.clone()), &amount);
}
