// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Pause and resume, revoke, and the three ways funds go back to the
//! principal: withdraw, sweep and token recovery.
//!
//! Revoke is a pure state change. It reads the balance and moves nothing,
//! so no issuer, missing trustline or token bug can stop a principal from
//! freezing its mandate. Funds then leave only through `withdraw` and
//! `sweep`, and neither takes a destination: both pay the principal, so a
//! phished signature or a stranger's sweep can never redirect a unit.
//!
//! Every transfer out goes through the token client's `try_` call and any
//! failure, whatever the token reports, becomes `TRANSFER_FAILED` with
//! nothing changed, so the call can simply be retried. The mandate never
//! reads a token's own error code: the asset contract's codes 10 to 12
//! would otherwise read as the mandate's role errors.

use crate::derived;
use crate::events::{Paused, Resumed, Revoked, Swept, TokenRecovered, Withdrawn};
use crate::payout::{ensure_not_revoked, SWEEP_GRACE_SECONDS};
use crate::storage::{self, Totals};
use crate::{UjiraMandate, UjiraMandateArgs, UjiraMandateClient};
use mandate_errors::MandateError;
use soroban_sdk::{contractimpl, token::TokenClient, Address, Env};

#[contractimpl]
impl UjiraMandate {
    /// Stops requests, approvals and executions. The principal, an
    /// approver, a guardian or a recovery approver may pause.
    pub fn pause(env: Env, caller: Address) -> Result<(), MandateError> {
        storage::extend_instance(&env);
        pause(&env, &caller)
    }

    /// Lifts a pause. The principal only.
    pub fn resume(env: Env) -> Result<(), MandateError> {
        storage::extend_instance(&env);
        resume(&env)
    }

    /// Ends the mandate for good and returns the balance it froze. Moves
    /// no funds.
    pub fn revoke(env: Env) -> Result<i128, MandateError> {
        storage::extend_instance(&env);
        revoke(&env)
    }

    /// Pays `amount` of the free balance to the principal.
    pub fn withdraw(env: Env, amount: i128) -> Result<(), MandateError> {
        storage::extend_instance(&env);
        withdraw(&env, amount)
    }

    /// Pays the whole balance of a closed mandate to the principal and
    /// returns the amount. Anyone may call it, inside the sweep guards.
    pub fn sweep(env: Env, caller: Address) -> Result<i128, MandateError> {
        storage::extend_instance(&env);
        sweep(&env, &caller)
    }

    /// Pays `amount` of a token other than the settlement asset to the
    /// principal.
    pub fn recover_token(env: Env, token: Address, amount: i128) -> Result<(), MandateError> {
        storage::extend_instance(&env);
        recover_token(&env, &token, amount)
    }
}

/// The principal, after its authorization. Every principal-only call starts
/// here, so another signer fails host authorization before any check runs.
pub(crate) fn authorized_principal(env: &Env) -> Result<Address, MandateError> {
    let principal = storage::principal(env)?;
    principal.require_auth();
    Ok(principal)
}

pub fn pause(env: &Env, caller: &Address) -> Result<(), MandateError> {
    caller.require_auth();
    let roles = storage::roles(env)?;
    let may = *caller == storage::principal(env)?
        || roles.is_approver(caller)
        || roles.guardians.contains(caller)
        || roles.recovery.contains(caller);
    if !may {
        return Err(MandateError::Unauthorized);
    }
    ensure_not_revoked(env)?;
    let mut flags = storage::flags(env)?;
    if flags.paused {
        return Err(MandateError::AlreadyPaused);
    }
    flags.paused = true;
    storage::set_flags(env, &flags);
    Paused {
        tenant_tag: storage::fixed(env)?.tenant_tag,
        by: caller.clone(),
    }
    .publish(env);
    Ok(())
}

pub fn resume(env: &Env) -> Result<(), MandateError> {
    let principal = authorized_principal(env)?;
    ensure_not_revoked(env)?;
    let mut flags = storage::flags(env)?;
    if !flags.paused {
        return Err(MandateError::NotPaused);
    }
    flags.paused = false;
    storage::set_flags(env, &flags);
    Resumed {
        tenant_tag: storage::fixed(env)?.tenant_tag,
        by: principal,
    }
    .publish(env);
    Ok(())
}

pub fn revoke(env: &Env) -> Result<i128, MandateError> {
    let principal = authorized_principal(env)?;
    ensure_not_revoked(env)?;
    revoke_effects(env, &principal)
}

/// What a revoke does, by the principal or by the recovery set: set
/// `revoked`, drop every reservation and report the balance. Role storage
/// is kept, so the history stays readable and the cost does not depend on
/// the size of the role sets; every role-gated path checks `revoked` first,
/// so every power ends here all the same. The only token call is the
/// balance read.
pub(crate) fn revoke_effects(env: &Env, by: &Address) -> Result<i128, MandateError> {
    let fixed = storage::fixed(env)?;
    let mut flags = storage::flags(env)?;
    flags.revoked = true;
    storage::set_flags(env, &flags);
    let totals = storage::totals(env)?;
    storage::set_totals(
        env,
        &Totals {
            spent: totals.spent,
            pending: 0,
            committed: 0,
        },
    );
    let frozen = derived::balance(env, &fixed.asset);
    Revoked {
        tenant_tag: fixed.tenant_tag,
        by: by.clone(),
        frozen,
    }
    .publish(env);
    Ok(frozen)
}

/// Allowed in every state. While the mandate is active, reserved funds are
/// not free, so an approved payout never fails at execute because money
/// quietly left; once it is revoked or expired the whole balance is free.
pub fn withdraw(env: &Env, amount: i128) -> Result<(), MandateError> {
    let principal = authorized_principal(env)?;
    if amount < 1 {
        return Err(MandateError::AmountInvalid);
    }
    let fixed = storage::fixed(env)?;
    let flags = storage::flags(env)?;
    let now = env.ledger().timestamp();
    let active = derived::is_active(flags.revoked, now, storage::limits(env)?.expires_at);
    let balance = derived::balance(env, &fixed.asset);
    if amount > derived::free(balance, &storage::totals(env)?, active)? {
        return Err(MandateError::InsufficientFunds);
    }
    transfer_out(env, &fixed.asset, &principal, amount)?;
    Withdrawn {
        tenant_tag: fixed.tenant_tag,
        to: principal,
        amount,
    }
    .publish(env);
    Ok(())
}

/// Returns the balance of a closed mandate to the principal. The caller is
/// authorized first, so nobody can borrow the principal's address to get
/// past the guards. A stranger waits while a principal change is pending,
/// so the funds never go to an address that is being abandoned, and waits
/// out the grace after expiry, which gives the principal time to extend or
/// withdraw. After a revoke there is no grace.
pub fn sweep(env: &Env, caller: &Address) -> Result<i128, MandateError> {
    caller.require_auth();
    let flags = storage::flags(env)?;
    let expires_at = storage::limits(env)?.expires_at;
    let now = env.ledger().timestamp();
    if derived::is_active(flags.revoked, now, expires_at) {
        return Err(MandateError::MandateActive);
    }
    let principal = storage::principal(env)?;
    let stranger = *caller != principal;
    if stranger && storage::proposed_principal(env).is_some() {
        return Err(MandateError::PrincipalChangePending);
    }
    if stranger && !flags.revoked {
        // Not active and not revoked means expired, so now >= expires_at.
        let since = now
            .checked_sub(expires_at)
            .ok_or(MandateError::InternalInvariant)?;
        if since < SWEEP_GRACE_SECONDS {
            return Err(MandateError::SweepGracePeriod);
        }
    }
    let fixed = storage::fixed(env)?;
    let balance = derived::balance(env, &fixed.asset);
    if balance <= 0 {
        return Err(MandateError::NothingToSweep);
    }
    transfer_out(env, &fixed.asset, &principal, balance)?;
    Swept {
        tenant_tag: fixed.tenant_tag,
        by: caller.clone(),
        amount: balance,
    }
    .publish(env);
    Ok(balance)
}

/// Returns a token that is not the settlement asset, in every state. The
/// settlement balance is read before and after the transfer: a token that
/// moved it, as a second address for the same asset could, would let this
/// call get around the free-balance cap of `withdraw`, so the whole call is
/// refused.
pub fn recover_token(env: &Env, token: &Address, amount: i128) -> Result<(), MandateError> {
    let principal = authorized_principal(env)?;
    let fixed = storage::fixed(env)?;
    if *token == fixed.asset {
        return Err(MandateError::WrongAsset);
    }
    if amount < 1 {
        return Err(MandateError::AmountInvalid);
    }
    let before = derived::balance(env, &fixed.asset);
    transfer_out(env, token, &principal, amount)?;
    if derived::balance(env, &fixed.asset) != before {
        return Err(MandateError::WrongAsset);
    }
    TokenRecovered {
        tenant_tag: fixed.tenant_tag,
        token: token.clone(),
        amount,
    }
    .publish(env);
    Ok(())
}

/// One transfer out of the mandate. Anything but a clean unit return,
/// whether a token error, a host error the call can catch or a value where
/// none belongs, is `TRANSFER_FAILED`, and the host rolls back whatever
/// the token did before failing.
fn transfer_out(
    env: &Env,
    token: &Address,
    to: &Address,
    amount: i128,
) -> Result<(), MandateError> {
    let client = TokenClient::new(env, token);
    match client.try_transfer(&env.current_contract_address(), to, &amount) {
        Ok(Ok(())) => Ok(()),
        _ => Err(MandateError::TransferFailed),
    }
}
