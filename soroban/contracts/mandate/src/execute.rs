// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Execution: the only path by which a payout moves the settlement asset.
//!
//! Execute re-checks everything that can change between request and now:
//! the payout's status and lapse, its agent, the fee cap, the per-payment
//! cap, the recipient binding, the daily headroom and the balance. Then it
//! writes every effect and emits its event before the first transfer, and
//! only then pays each leg and, last, the fee.
//!
//! Every transfer is a guarded call. Whatever the token returns that is not
//! a plain success becomes `TRANSFER_FAILED`, and returning that error makes
//! the host undo the whole call: the tombstone, the counters, the event and
//! any leg already paid. So a failed execute leaves the payout APPROVED and
//! a retry pays it exactly once. A token error is never passed through,
//! because the asset contract's codes 10 to 12 would read as mandate codes.

use crate::derived;
use crate::events::PayoutExecuted;
use crate::payout::{
    charged, ensure_active, fee_cap, invariant, open_record, release, same_agent, MAX_BATCH,
};
use crate::storage::{self, Fixed, Roles};
use crate::types::{DailyUsage, PayoutRecord, STATUS_APPROVED, STATUS_EXECUTED};
use mandate_config::{MODE_ALLOWLIST, MODE_ATTESTED, MODE_BOTH};
use mandate_errors::MandateError;
use soroban_sdk::{symbol_short, Address, BytesN, Env, IntoVal, InvokeError, MuxedAddress, Vec};

/// Token transfers per `execute_batch`: every leg of every item, plus one
/// per item that carries a fee. The bound comes from the per-transaction
/// events limit, since every transfer emits an event the mandate cannot
/// avoid.
pub const MAX_BATCH_TRANSFERS: u32 = 18;

/// The principal, any current agent or an executor may execute. An agent may
/// execute another agent's payout: what is paid was decided at request and
/// approval, and execute re-checks all of it.
fn ensure_may_execute(env: &Env, caller: &Address, roles: &Roles) -> Result<(), MandateError> {
    if roles.agent(caller).is_some()
        || roles.executors.contains(caller)
        || *caller == storage::principal(env)?
    {
        return Ok(());
    }
    Err(MandateError::Unauthorized)
}

pub fn execute(env: &Env, caller: &Address, payout_id: &BytesN<32>) -> Result<(), MandateError> {
    caller.require_auth();
    let roles = storage::roles(env)?;
    ensure_may_execute(env, caller, &roles)?;
    let now = env.ledger().timestamp();
    ensure_active(env, now)?;
    let fixed = storage::fixed(env)?;
    let record = check_item(env, &fixed, &roles, payout_id, now)?;
    pay(env, &fixed, &record, now)
}

/// Atomic: each item is checked against what the earlier items left, then
/// paid, before the next item is looked at, and the first refusal reverts
/// the whole batch. A batch never leaves part of its work behind a caught
/// transfer failure.
pub fn execute_batch(
    env: &Env,
    caller: &Address,
    ids: &Vec<BytesN<32>>,
) -> Result<(), MandateError> {
    caller.require_auth();
    let roles = storage::roles(env)?;
    ensure_may_execute(env, caller, &roles)?;
    let now = env.ledger().timestamp();
    ensure_active(env, now)?;
    if ids.is_empty() || ids.len() > MAX_BATCH {
        return Err(MandateError::BatchInvalid);
    }
    let fixed = storage::fixed(env)?;
    let mut transfers: u32 = 0;
    for id in ids.iter() {
        let record = check_item(env, &fixed, &roles, &id, now)?;
        // Counted after the item's own checks, so an item that breaks a rule
        // reports that rule and never the budget.
        transfers = invariant(transfers.checked_add(transfer_count(&record)?))?;
        if transfers > MAX_BATCH_TRANSFERS {
            return Err(MandateError::BatchInvalid);
        }
        pay(env, &fixed, &record, now)?;
    }
    Ok(())
}

/// The legs, plus one when there is a fee to send.
fn transfer_count(record: &PayoutRecord) -> Result<u32, MandateError> {
    let legs = record.legs.len();
    if record.fee > 0 {
        invariant(legs.checked_add(1))
    } else {
        Ok(legs)
    }
}

/// The execute checks from the lookup onward, in their fixed order. The
/// roles come from the caller, read once, since paying an item never
/// changes them; the counters, the day usage and the balance are read
/// fresh for every item, since paying one does.
fn check_item(
    env: &Env,
    fixed: &Fixed,
    roles: &Roles,
    payout_id: &BytesN<32>,
    now: u64,
) -> Result<PayoutRecord, MandateError> {
    let record = open_record(env, payout_id)?;
    if record.status != STATUS_APPROVED {
        return Err(MandateError::PayoutWrongStatus);
    }
    if now >= record.expires_at {
        return Err(MandateError::PayoutLapsed);
    }
    // Taking an agent away, or rotating it out, stops everything it queued,
    // approved or not.
    if !same_agent(roles, &record) {
        return Err(MandateError::AgentRemoved);
    }
    let policy = storage::policy(env)?;
    // Both caps are the ones in force now, so lowering either also stops
    // payouts already approved.
    if record.fee > fee_cap(record.total, policy.max_fee_bps)? {
        return Err(MandateError::FeeExceedsCap);
    }
    let charge = charged(record.total, record.fee)?;
    let limits = storage::limits(env)?;
    if charge > limits.per_payment_cap {
        return Err(MandateError::PaymentCapExceeded);
    }
    let mode = fixed.recipient_mode;
    if (mode == MODE_ALLOWLIST || mode == MODE_BOTH)
        && record
            .legs
            .iter()
            .any(|leg| !storage::is_allowed(env, &record.recipient_ref, &leg.destination))
    {
        return Err(MandateError::RecipientNotAllowed);
    }
    // Attestations are not kept, so a change of attester or country policy
    // is caught by its epoch.
    if (mode == MODE_ATTESTED || mode == MODE_BOTH) && record.policy_epoch != policy.policy_epoch {
        return Err(MandateError::PolicyChanged);
    }
    // The binding daily check: against what was actually paid today, which
    // includes earlier items of the same batch.
    let used = used_today(env, &record.recipient_ref, now);
    let headroom = if used >= limits.per_recipient_daily_cap {
        0
    } else {
        invariant(limits.per_recipient_daily_cap.checked_sub(used))?
    };
    if record.total > headroom {
        return Err(MandateError::DailyCapExceeded);
    }
    // The charge was reserved at request, so this compares with the whole
    // balance, not the free part. Last, because it calls the token.
    if charge > derived::balance(env, &fixed.asset) {
        return Err(MandateError::InsufficientFunds);
    }
    Ok(record)
}

fn used_today(env: &Env, recipient_ref: &BytesN<32>, now: u64) -> i128 {
    let today = derived::day_of(now);
    match storage::daily(env, recipient_ref) {
        Some(usage) if usage.day == today => usage.used,
        _ => 0,
    }
}

/// The effects, then the transfers. State and the event come first so that
/// nothing a token does during a transfer can see this payout still open.
fn pay(env: &Env, fixed: &Fixed, record: &PayoutRecord, now: u64) -> Result<(), MandateError> {
    let charge = charged(record.total, record.fee)?;
    let mut totals = storage::totals(env)?;
    release(&mut totals, STATUS_APPROVED, charge)?;
    totals.spent = invariant(totals.spent.checked_add(charge))?;
    let today = derived::day_of(now);
    let used = invariant(used_today(env, &record.recipient_ref, now).checked_add(record.total))?;

    // The tombstone keeps the record's remaining TTL and is left to archive.
    storage::put_tombstone(env, &record.payout_id, STATUS_EXECUTED);
    storage::set_totals(env, &totals);
    storage::set_daily(env, &record.recipient_ref, &DailyUsage { day: today, used });
    PayoutExecuted {
        payout_id: record.payout_id.clone(),
        recipient_ref: record.recipient_ref.clone(),
        total: record.total,
        fee: record.fee,
        legs: record.legs.clone(),
        day: today,
        spent: totals.spent,
    }
    .publish(env);

    for leg in record.legs.iter() {
        transfer(env, &fixed.asset, &leg.destination, leg.amount)?;
    }
    // The fee goes last, after every leg, and only here.
    if record.fee > 0 {
        transfer(
            env,
            &fixed.asset,
            &MuxedAddress::from(fixed.fee_address.clone()),
            record.fee,
        )?;
    }
    Ok(())
}

/// Sends `amount` of the asset from the mandate to `to`, tag included, as a
/// guarded call. Any error of any type, and any return other than unit, is
/// `TRANSFER_FAILED`; the token's own code never escapes.
pub(crate) fn transfer(
    env: &Env,
    asset: &Address,
    to: &MuxedAddress,
    amount: i128,
) -> Result<(), MandateError> {
    let args = (env.current_contract_address(), to.clone(), amount).into_val(env);
    match env.try_invoke_contract::<(), InvokeError>(asset, &symbol_short!("transfer"), args) {
        Ok(Ok(())) => Ok(()),
        _ => Err(MandateError::TransferFailed),
    }
}
