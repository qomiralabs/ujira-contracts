// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The UJIRA mandate.
//!
//! A mandate holds one principal's settlement asset and lets agent keys pay
//! recipients only inside the limits the principal set. Funds only ever
//! leave to destinations bound to a recipient, or back to the principal.
//!
//! The constructor validates and stores the configuration; agents request
//! payouts, approvers approve or reject them, the principal, an agent or an
//! executor executes the approved ones, and open ones can be cancelled.
//! The contract is never upgradeable; there is no call that replaces its
//! code.
//!
//! - `types`: the values the mandate takes and answers with.
//! - `storage`: where each value lives and for how long.
//! - `derived`: balance, expiry and free funds, computed on read.
//! - `events`: the event shapes.
//! - `payout`: requests, approvals, rejections and cancels.
//! - `execute`: paying approved payouts, alone or in an atomic batch.
//! - `attest`: recipient attestations and the message the attester signs.
//! - `lifecycle`: pause, resume, revoke, withdraw, sweep and token recovery.
//! - `admin`: limits, roles, the allowlist, the attestation policy, the
//!   principal transfer and the fee cap.
//! - `recovery`: the recovery set's votes to revoke or rotate an agent.
//!
//! The last three each declare their own entry points next to their logic.

#![no_std]

pub mod admin;
pub mod attest;
pub mod derived;
pub mod events;
pub mod execute;
pub mod lifecycle;
pub mod payout;
pub mod recovery;
pub mod storage;
pub mod types;

pub use mandate_config::{Limits, MandateConfig, SPEC_VERSION};
pub use mandate_errors::MandateError;

use events::MandateInitialized;
use soroban_sdk::{contract, contractimpl, Address, BytesN, Env, MuxedAddress, Vec};
use storage::{Agent, Fixed, Flags, Policy, Roles, Totals};
use types::{ConfigView, DailyUsage, ItemResult, PayoutEntry, PayoutRequest, StateView};

#[contract]
pub struct UjiraMandate;

#[contractimpl]
impl UjiraMandate {
    /// Runs once, inside the factory's deploy. `fee_address` comes from the
    /// factory, never from the principal. The factory has already run the
    /// same validation and returned its own error; running it again here
    /// means no path can store a configuration the rules refuse.
    ///
    /// The constructor cannot prove who deployed it: a deploying contract is
    /// not the direct caller of the constructor, so `factory.require_auth()`
    /// would refuse every real deploy. A mandate is trusted because its
    /// address derives from the factory and the salt, and the factory emits
    /// `mandate_created` for it, never because of the `factory` it stores.
    pub fn __constructor(
        env: Env,
        factory: Address,
        fee_address: Address,
        config: MandateConfig,
    ) -> Result<(), MandateError> {
        mandate_config::validate(&env, &config, &fee_address)?;

        storage::set_fixed(
            &env,
            &Fixed {
                factory,
                asset: config.asset.clone(),
                recipient_mode: config.recipient_mode,
                tenant_ref: config.tenant_ref.clone(),
                fee_address: fee_address.clone(),
            },
        );
        storage::set_principal(&env, &config.principal);
        storage::set_limits(&env, &config.limits);
        storage::set_policy(
            &env,
            &Policy {
                attester: config.attester.clone(),
                country_policy_hash: config.country_policy_hash.clone(),
                policy_epoch: 0,
                max_fee_bps: config.max_fee_bps,
            },
        );
        // Agents named at creation hold epoch 0; every later addition takes
        // a higher value, so no payout queued before it can match it.
        let mut agents = Vec::new(&env);
        for account in config.agents.iter() {
            agents.push_back(Agent { account, epoch: 0 });
        }
        storage::set_roles(
            &env,
            &Roles {
                agents,
                approvers: config.approvers.clone(),
                approvals_required: config.approvals_required,
                guardians: config.guardians.clone(),
                executors: config.executors.clone(),
                recovery: config.recovery.clone(),
                recovery_threshold: config.recovery_threshold,
                agent_epoch: 0,
                recovery_epoch: 0,
            },
        );
        storage::set_totals(
            &env,
            &Totals {
                spent: 0,
                pending: 0,
                committed: 0,
            },
        );
        storage::set_flags(
            &env,
            &Flags {
                paused: false,
                revoked: false,
            },
        );
        storage::extend_instance(&env);

        MandateInitialized {
            tenant_ref: config.tenant_ref,
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
        Ok(())
    }

    /// Requests one payout as the agent `caller`. Returns the status the
    /// payout starts in: PENDING when its charge reaches the approval
    /// threshold, APPROVED otherwise.
    pub fn request_payout(
        env: Env,
        caller: Address,
        req: PayoutRequest,
    ) -> Result<u32, MandateError> {
        storage::extend_instance(&env);
        payout::request_payout(&env, &caller, &req)
    }

    /// Requests up to ten payouts, each on its own: one result per item, in
    /// order, with code 0 for success.
    pub fn request_payouts(
        env: Env,
        caller: Address,
        reqs: Vec<PayoutRequest>,
    ) -> Result<Vec<ItemResult>, MandateError> {
        storage::extend_instance(&env);
        payout::request_payouts(&env, &caller, &reqs)
    }

    /// Adds the approver `caller`'s approval and returns the status after it.
    pub fn approve(env: Env, caller: Address, payout_id: BytesN<32>) -> Result<u32, MandateError> {
        storage::extend_instance(&env);
        payout::approve(&env, &caller, &payout_id)
    }

    /// Vetoes a pending payout, as an approver or the principal.
    pub fn reject(env: Env, caller: Address, payout_id: BytesN<32>) -> Result<(), MandateError> {
        storage::extend_instance(&env);
        payout::reject(&env, &caller, &payout_id)
    }

    /// Cancels an open payout and releases what it reserved.
    pub fn cancel(env: Env, caller: Address, payout_id: BytesN<32>) -> Result<(), MandateError> {
        storage::extend_instance(&env);
        payout::cancel(&env, &caller, &payout_id)
    }

    /// Cancels up to ten payouts, each on its own.
    pub fn cancel_batch(
        env: Env,
        caller: Address,
        ids: Vec<BytesN<32>>,
    ) -> Result<Vec<ItemResult>, MandateError> {
        storage::extend_instance(&env);
        payout::cancel_batch(&env, &caller, &ids)
    }

    /// Pays an approved payout: every leg to its destination, tag included,
    /// then the fee. On any failure nothing is paid and nothing changes, so
    /// the same call can simply be retried.
    pub fn execute(env: Env, caller: Address, payout_id: BytesN<32>) -> Result<(), MandateError> {
        storage::extend_instance(&env);
        execute::execute(&env, &caller, &payout_id)
    }

    /// Pays up to ten approved payouts in order, all or none.
    pub fn execute_batch(
        env: Env,
        caller: Address,
        ids: Vec<BytesN<32>>,
    ) -> Result<(), MandateError> {
        storage::extend_instance(&env);
        execute::execute_batch(&env, &caller, &ids)
    }

    pub fn version(env: Env) -> u32 {
        storage::extend_instance(&env);
        SPEC_VERSION
    }

    /// The configuration as it stands now, with the fee address, any
    /// proposed principal and the recovery and agent epochs.
    pub fn config(env: Env) -> Result<ConfigView, MandateError> {
        storage::extend_instance(&env);
        let fixed = storage::fixed(&env)?;
        let policy = storage::policy(&env)?;
        let roles = storage::roles(&env)?;
        Ok(ConfigView {
            principal: storage::principal(&env)?,
            asset: fixed.asset,
            recipient_mode: fixed.recipient_mode,
            tenant_ref: fixed.tenant_ref,
            agents: roles.agent_addresses(&env),
            approvers: roles.approvers,
            approvals_required: roles.approvals_required,
            guardians: roles.guardians,
            executors: roles.executors,
            recovery: roles.recovery,
            recovery_threshold: roles.recovery_threshold,
            limits: storage::limits(&env)?,
            attester: policy.attester,
            country_policy_hash: policy.country_policy_hash,
            max_fee_bps: policy.max_fee_bps,
            fee_address: fixed.fee_address,
            proposed_principal: storage::proposed_principal(&env),
            recovery_epoch: roles.recovery_epoch,
            agent_epoch: roles.agent_epoch,
        })
    }

    /// The counters with the balance, free funds and expiry derived from
    /// the token and the ledger time at this moment.
    pub fn state(env: Env) -> Result<StateView, MandateError> {
        storage::extend_instance(&env);
        let fixed = storage::fixed(&env)?;
        let limits = storage::limits(&env)?;
        let totals = storage::totals(&env)?;
        let flags = storage::flags(&env)?;
        let policy = storage::policy(&env)?;
        let now = env.ledger().timestamp();
        let balance = derived::balance(&env, &fixed.asset);
        let active = derived::is_active(flags.revoked, now, limits.expires_at);
        let free = derived::free(balance, &totals, active)?;
        Ok(StateView {
            spent: totals.spent,
            pending: totals.pending,
            committed: totals.committed,
            balance,
            free,
            paused: flags.paused,
            revoked: flags.revoked,
            expired: derived::is_expired(now, limits.expires_at),
            policy_epoch: policy.policy_epoch,
        })
    }

    /// The record while the payout is open, its tombstone once it is
    /// terminal, or None if the ID was never used.
    pub fn payout(env: Env, payout_id: BytesN<32>) -> Option<PayoutEntry> {
        storage::extend_instance(&env);
        storage::payout(&env, &payout_id)
    }

    /// Today's executed total for a recipient. A stored day that is not
    /// today counts as nothing used today.
    pub fn daily_used(env: Env, recipient_ref: BytesN<32>) -> DailyUsage {
        storage::extend_instance(&env);
        let today = derived::day_of(env.ledger().timestamp());
        match storage::daily(&env, &recipient_ref) {
            Some(usage) if usage.day == today => usage,
            _ => DailyUsage {
                day: today,
                used: 0,
            },
        }
    }

    /// Whether the destination, tag included, is allowed for the recipient.
    pub fn is_allowed(env: Env, recipient_ref: BytesN<32>, destination: MuxedAddress) -> bool {
        storage::extend_instance(&env);
        storage::is_allowed(&env, &recipient_ref, &destination)
    }
}

#[cfg(test)]
mod test;
