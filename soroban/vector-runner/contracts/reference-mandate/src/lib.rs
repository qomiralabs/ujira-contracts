// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Test only, never deployed. A stand-in for the mandate that the vector
//! runner's self tests drive, so the runner, its decoders and its comparator
//! can be proven before the real contract exists.
//!
//! It speaks the same interface the runner expects from the real mandate for
//! the parts it implements: the constructor the factory calls, `version`,
//! `state`, `config`, `pause` and `resume`, with the spec's event shapes. It
//! keeps no payouts, so every amount but the balance is zero.
//!
//! Two answers are wrong on purpose, so the self tests can show the runner
//! catches them:
//!
//! - `version` answers 2 where the spec says 1.
//! - `withdraw` does not wrap its token transfer, so a failed transfer
//!   escapes with the token's own error code. The Stellar Asset Contract's
//!   codes 10 to 12 are also mandate codes, and the runner must report such a
//!   failure as the token's, never as the mandate code with the same number.

#![no_std]

use mandate_errors::MandateError as Error;
use soroban_sdk::{
    contract, contractevent, contractimpl, contracttype, panic_with_error, token::TokenClient,
    Address, Bytes, BytesN, Env, Vec,
};

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
    pub tenant_tag: BytesN<32>,
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

/// What `config()` answers: the configuration plus what the mandate keeps
/// beside it.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigView {
    pub principal: Address,
    pub asset: Address,
    pub recipient_mode: u32,
    pub tenant_tag: BytesN<32>,
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
    pub fee_address: Address,
    pub proposed_principal: Option<Address>,
    pub recovery_epoch: u32,
    pub agent_epoch: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateView {
    pub spent: i128,
    pub pending: i128,
    pub committed: i128,
    pub balance: i128,
    pub free: i128,
    pub paused: bool,
    pub revoked: bool,
    pub expired: bool,
    pub policy_epoch: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
enum Key {
    Config,
    FeeAddress,
    Paused,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MandateInitialized {
    #[topic]
    pub tenant_tag: BytesN<32>,
    pub principal: Address,
    pub asset: Address,
    pub recipient_mode: u32,
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
    pub fee_address: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Paused {
    #[topic]
    pub tenant_tag: BytesN<32>,
    pub by: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Resumed {
    #[topic]
    pub tenant_tag: BytesN<32>,
    pub by: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Withdrawn {
    #[topic]
    pub tenant_tag: BytesN<32>,
    pub to: Address,
    pub amount: i128,
}

#[contract]
pub struct ReferenceMandate;

#[contractimpl]
impl ReferenceMandate {
    pub fn __constructor(env: Env, _factory: Address, fee_address: Address, config: MandateConfig) {
        let store = env.storage().instance();
        store.set(&Key::Config, &config);
        store.set(&Key::FeeAddress, &fee_address);
        store.set(&Key::Paused, &false);
        MandateInitialized {
            tenant_tag: config.tenant_tag,
            principal: config.principal,
            asset: config.asset,
            recipient_mode: config.recipient_mode,
            agents: config.agents,
            approvers: config.approvers,
            approvals_required: config.approvals_required,
            guardians: config.guardians,
            executors: config.executors,
            recovery: config.recovery,
            recovery_threshold: config.recovery_threshold,
            limits: config.limits,
            attester: config.attester,
            country_policy_hash: config.country_policy_hash,
            max_fee_bps: config.max_fee_bps,
            fee_address,
        }
        .publish(&env);
    }

    /// Deliberately wrong: the spec version is 1.
    pub fn version() -> u32 {
        2
    }

    pub fn state(env: Env) -> StateView {
        let cfg = load_config(&env);
        let balance = TokenClient::new(&env, &cfg.asset).balance(&env.current_contract_address());
        let expired = env.ledger().timestamp() >= cfg.limits.expires_at;
        // Nothing is ever reserved here, so the free balance is the balance
        // whether or not the mandate is active.
        StateView {
            spent: 0,
            pending: 0,
            committed: 0,
            balance,
            free: balance,
            paused: is_paused(&env),
            revoked: false,
            expired,
            policy_epoch: 0,
        }
    }

    pub fn config(env: Env) -> ConfigView {
        let cfg = load_config(&env);
        let fee_address = match env.storage().instance().get(&Key::FeeAddress) {
            Some(a) => a,
            None => panic_with_error!(&env, Error::InternalInvariant),
        };
        ConfigView {
            principal: cfg.principal,
            asset: cfg.asset,
            recipient_mode: cfg.recipient_mode,
            tenant_tag: cfg.tenant_tag,
            agents: cfg.agents,
            approvers: cfg.approvers,
            approvals_required: cfg.approvals_required,
            guardians: cfg.guardians,
            executors: cfg.executors,
            recovery: cfg.recovery,
            recovery_threshold: cfg.recovery_threshold,
            limits: cfg.limits,
            attester: cfg.attester,
            country_policy_hash: cfg.country_policy_hash,
            max_fee_bps: cfg.max_fee_bps,
            fee_address,
            proposed_principal: None,
            recovery_epoch: 0,
            agent_epoch: 0,
        }
    }

    pub fn pause(env: Env, caller: Address) -> Result<(), Error> {
        caller.require_auth();
        let cfg = load_config(&env);
        let allowed = caller == cfg.principal
            || cfg.approvers.contains(&caller)
            || cfg.guardians.contains(&caller)
            || cfg.recovery.contains(&caller);
        if !allowed {
            return Err(Error::Unauthorized);
        }
        if is_paused(&env) {
            return Err(Error::AlreadyPaused);
        }
        env.storage().instance().set(&Key::Paused, &true);
        Paused {
            tenant_tag: cfg.tenant_tag,
            by: caller,
        }
        .publish(&env);
        Ok(())
    }

    pub fn resume(env: Env) -> Result<(), Error> {
        let cfg = load_config(&env);
        cfg.principal.require_auth();
        if !is_paused(&env) {
            return Err(Error::NotPaused);
        }
        env.storage().instance().set(&Key::Paused, &false);
        Resumed {
            tenant_tag: cfg.tenant_tag,
            by: cfg.principal,
        }
        .publish(&env);
        Ok(())
    }

    /// Deliberately unwrapped: a failed transfer escapes with the token's
    /// own error instead of TRANSFER_FAILED.
    pub fn withdraw(env: Env, amount: i128) -> Result<(), Error> {
        let cfg = load_config(&env);
        cfg.principal.require_auth();
        if amount < 1 {
            return Err(Error::AmountInvalid);
        }
        TokenClient::new(&env, &cfg.asset).transfer(
            &env.current_contract_address(),
            &cfg.principal,
            &amount,
        );
        Withdrawn {
            tenant_tag: cfg.tenant_tag,
            to: cfg.principal,
            amount,
        }
        .publish(&env);
        Ok(())
    }
}

fn load_config(env: &Env) -> MandateConfig {
    match env.storage().instance().get(&Key::Config) {
        Some(cfg) => cfg,
        None => panic_with_error!(env, Error::InternalInvariant),
    }
}

fn is_paused(env: &Env) -> bool {
    env.storage().instance().get(&Key::Paused).unwrap_or(false)
}
