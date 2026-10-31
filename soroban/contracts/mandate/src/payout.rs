// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Requests, approvals, rejections and cancels.
//!
//! Each operation runs its checks in one fixed order and the first failure
//! decides the error. Nothing is written or emitted until every check has
//! passed, so a refused call, or a refused item of a batch, leaves no trace.
//!
//! Amounts only ever meet addition, subtraction and comparison, in forms
//! that cannot overflow for any input the checks let through (the budget is
//! compared against what is left of it, never against a sum that could
//! wrap). The one product, the fee cap, is split so it cannot overflow
//! either. Every operation is still checked, and a failure is
//! `INTERNAL_INVARIANT` rather than a trap, so a broken invariant is a
//! readable code.

use crate::attest;
use crate::derived;
use crate::events::{
    PayoutApprovalAdded, PayoutApproved, PayoutCancelled, PayoutRejected, PayoutRequestRejected,
    PayoutRequested,
};
use crate::storage::{self, Agent, Policy, Roles, Totals};
use crate::types::{
    ItemResult, Leg, PayoutEntry, PayoutRecord, PayoutRequest, STATUS_APPROVED, STATUS_CANCELLED,
    STATUS_PENDING, STATUS_REJECTED,
};
use mandate_config::{MODE_ALLOWLIST, MODE_ATTESTED, MODE_BOTH};
use mandate_errors::MandateError;
use soroban_sdk::{Address, BytesN, Env, Vec};

/// Legs per payout.
pub const MAX_LEGS: u32 = 4;

/// Items per request or cancel batch.
pub const MAX_BATCH: u32 = 10;

/// How long after the mandate expires a stranger must wait before it may
/// cancel a payout that has not lapsed on its own.
pub const SWEEP_GRACE_SECONDS: u64 = 2_592_000;

/// Basis points per whole, for the fee cap.
pub const BPS_DENOMINATOR: i128 = 10_000;

/// Cancel reasons, as `payout_cancelled` carries them.
pub const CANCEL_BY_PRINCIPAL: u32 = 1;
pub const CANCEL_BY_AGENT: u32 = 2;
pub const CANCEL_LAPSED: u32 = 3;

/// The item code for success in a batch result.
const ITEM_OK: u32 = 0;

fn invariant<T>(v: Option<T>) -> Result<T, MandateError> {
    v.ok_or(MandateError::InternalInvariant)
}

/// The three active checks, in their fixed order.
fn ensure_active(env: &Env, now: u64) -> Result<(), MandateError> {
    let flags = storage::flags(env)?;
    if flags.revoked {
        return Err(MandateError::MandateRevoked);
    }
    if derived::is_expired(now, storage::limits(env)?.expires_at) {
        return Err(MandateError::MandateExpired);
    }
    if flags.paused {
        return Err(MandateError::MandatePaused);
    }
    Ok(())
}

fn ensure_not_revoked(env: &Env) -> Result<(), MandateError> {
    if storage::flags(env)?.revoked {
        return Err(MandateError::MandateRevoked);
    }
    Ok(())
}

/// The caller's agent entry, or `UNAUTHORIZED` when it is not an agent.
fn agent_entry(env: &Env, caller: &Address) -> Result<Agent, MandateError> {
    storage::roles(env)?
        .agent(caller)
        .ok_or(MandateError::Unauthorized)
}

/// True while the payout's agent is still an agent under the epoch it held
/// at request. A removal ends that, and so does adding the same address
/// back, since the new entry carries a later epoch.
fn same_agent(roles: &Roles, record: &PayoutRecord) -> bool {
    matches!(roles.agent(&record.agent), Some(a) if a.epoch == record.agent_epoch)
}

/// What a payout charges against the mandate: the total and the fee.
fn charged(total: i128, fee: i128) -> Result<i128, MandateError> {
    invariant(total.checked_add(fee))
}

/// The largest fee a total allows, rounded down. Equal to
/// `floor(total * bps / 10000)` without ever forming `total * bps`, which
/// would overflow near the largest amount.
pub fn fee_cap(total: i128, max_fee_bps: u32) -> Result<i128, MandateError> {
    let bps = i128::from(max_fee_bps);
    let whole = invariant(
        total
            .checked_div(BPS_DENOMINATOR)
            .and_then(|q| q.checked_mul(bps)),
    )?;
    let part = invariant(
        total
            .checked_rem(BPS_DENOMINATOR)
            .and_then(|r| r.checked_mul(bps))
            .and_then(|p| p.checked_div(BPS_DENOMINATOR)),
    )?;
    invariant(whole.checked_add(part))
}

/// The record behind a payout ID. A tombstone is a payout that already
/// reached a terminal status, so no call that takes an open payout accepts
/// it.
fn open_record(env: &Env, payout_id: &BytesN<32>) -> Result<PayoutRecord, MandateError> {
    match storage::payout(env, payout_id) {
        None => Err(MandateError::PayoutNotFound),
        Some(PayoutEntry::Tombstone(_)) => Err(MandateError::PayoutWrongStatus),
        Some(PayoutEntry::Record(r)) => Ok(r),
    }
}

/// Takes `amount` back out of `pending` or `committed`, whichever the
/// status reserved it in. Going below zero would mean the counters no
/// longer match the open payouts.
fn release(totals: &mut Totals, status: u32, amount: i128) -> Result<(), MandateError> {
    let slot = match status {
        STATUS_PENDING => &mut totals.pending,
        STATUS_APPROVED => &mut totals.committed,
        _ => return Err(MandateError::InternalInvariant),
    };
    let left = invariant(slot.checked_sub(amount))?;
    if left < 0 {
        return Err(MandateError::InternalInvariant);
    }
    *slot = left;
    Ok(())
}

// Requests.

pub fn request_payout(
    env: &Env,
    caller: &Address,
    req: &PayoutRequest,
) -> Result<u32, MandateError> {
    caller.require_auth();
    let agent = agent_entry(env, caller)?;
    ensure_active(env, env.ledger().timestamp())?;
    request_item(env, &agent, req)
}

/// Each item runs against the state the earlier items left, and a refused
/// item is reported and skipped rather than reverting the batch. The one
/// exception is a wrongly signed attestation, which traps in the host and
/// aborts everything.
pub fn request_payouts(
    env: &Env,
    caller: &Address,
    reqs: &Vec<PayoutRequest>,
) -> Result<Vec<ItemResult>, MandateError> {
    caller.require_auth();
    let agent = agent_entry(env, caller)?;
    ensure_active(env, env.ledger().timestamp())?;
    if reqs.is_empty() || reqs.len() > MAX_BATCH {
        return Err(MandateError::BatchInvalid);
    }
    let mut out = Vec::new(env);
    for req in reqs.iter() {
        let result = match request_item(env, &agent, &req) {
            Ok(status) => ItemResult {
                payout_id: req.payout_id,
                code: ITEM_OK,
                status: Some(status),
            },
            Err(e) => {
                let code = e as u32;
                PayoutRequestRejected {
                    payout_id: req.payout_id.clone(),
                    agent: caller.clone(),
                    code,
                }
                .publish(env);
                ItemResult {
                    payout_id: req.payout_id,
                    code,
                    status: None,
                }
            }
        };
        out.push_back(result);
    }
    Ok(out)
}

/// The request checks from the payout ID onward, then the effects. Shared
/// by the single and the batch call, which have already checked the caller
/// and that the mandate is active.
fn request_item(env: &Env, agent: &Agent, req: &PayoutRequest) -> Result<u32, MandateError> {
    let now = env.ledger().timestamp();

    // Early, so a retry of a request that already landed gets the most
    // useful answer.
    storage::ensure_payout_id_unused(env, &req.payout_id)?;
    if now >= req.deadline {
        return Err(MandateError::RequestDeadlinePassed);
    }
    let charge = check_amounts(req.total, req.fee)?;

    let fixed = storage::fixed(env)?;
    check_legs(&req.legs, req.total, &fixed.fee_address)?;

    let policy = storage::policy(env)?;
    let earliest_valid_until = check_recipients(env, fixed.recipient_mode, &policy, req, now)?;

    if req.fee > fee_cap(req.total, policy.max_fee_bps)? {
        return Err(MandateError::FeeExceedsCap);
    }
    let limits = storage::limits(env)?;
    if charge > limits.per_payment_cap {
        return Err(MandateError::PaymentCapExceeded);
    }
    let mut totals = storage::totals(env)?;
    let reserved = invariant(
        totals
            .spent
            .checked_add(totals.pending)
            .and_then(|s| s.checked_add(totals.committed)),
    )?;
    // What is left of the budget, compared with the charge, so that a
    // charge near the largest amount can never overflow a sum.
    let budget_left = invariant(limits.total_budget.checked_sub(reserved))?;
    if charge > budget_left {
        return Err(MandateError::BudgetExceeded);
    }
    let today = derived::day_of(now);
    let used = match storage::daily(env, &req.recipient_ref) {
        Some(usage) if usage.day == today => usage.used,
        _ => 0,
    };
    let headroom = if used >= limits.per_recipient_daily_cap {
        0
    } else {
        invariant(limits.per_recipient_daily_cap.checked_sub(used))?
    };
    // The daily cap is a limit on what a person receives, so it counts the
    // total and never the fee.
    if req.total > headroom {
        return Err(MandateError::DailyCapExceeded);
    }
    // Last, because it is the one check that calls the token.
    let balance = derived::balance(env, &fixed.asset);
    if charge > derived::free(balance, &totals, true)? {
        return Err(MandateError::InsufficientFunds);
    }

    // Every check passed. Funds are reserved now, so `committed` always
    // means covered by the balance and the principal sees what is queued.
    let status = if charge >= limits.approval_threshold {
        totals.pending = invariant(totals.pending.checked_add(charge))?;
        STATUS_PENDING
    } else {
        totals.committed = invariant(totals.committed.checked_add(charge))?;
        STATUS_APPROVED
    };
    let mut expires_at = invariant(now.checked_add(limits.payout_ttl_seconds))?;
    // An attested payout cannot outlive the statement that it may be paid.
    if let Some(v) = earliest_valid_until {
        expires_at = expires_at.min(v);
    }
    let record = PayoutRecord {
        payout_id: req.payout_id.clone(),
        agent: agent.account.clone(),
        agent_epoch: agent.epoch,
        recipient_ref: req.recipient_ref.clone(),
        total: req.total,
        fee: req.fee,
        legs: req.legs.clone(),
        status,
        created_at: now,
        expires_at,
        policy_epoch: policy.policy_epoch,
        approvals: Vec::new(env),
    };
    storage::put_new_record(env, &record);
    storage::set_totals(env, &totals);
    PayoutRequested {
        payout_id: req.payout_id.clone(),
        agent: agent.account.clone(),
        recipient_ref: req.recipient_ref.clone(),
        total: req.total,
        fee: req.fee,
        legs: req.legs.clone(),
        task_ref: req.task_ref.clone(),
        status,
        expires_at,
    }
    .publish(env);
    Ok(status)
}

/// The total is at least 1, the fee at least 0, and their sum fits in an
/// `i128`. Returns the sum, the charge. `I128_MAX - total` cannot overflow
/// once the total is known to be positive.
fn check_amounts(total: i128, fee: i128) -> Result<i128, MandateError> {
    if total < 1 || fee < 0 {
        return Err(MandateError::AmountInvalid);
    }
    let room = invariant(i128::MAX.checked_sub(total))?;
    if fee > room {
        return Err(MandateError::AmountInvalid);
    }
    charged(total, fee)
}

fn same_destination(a: &Leg, b: &Leg) -> bool {
    a.destination.address() == b.destination.address() && a.destination.id() == b.destination.id()
}

/// Leg count, leg amounts, the fee address, duplicates and the sum, in that
/// order. Stellar has no zero address, and a muxed address cannot mark a
/// tag absent while carrying a value, so those two leg rules have nothing
/// to check here.
fn check_legs(legs: &Vec<Leg>, total: i128, fee_address: &Address) -> Result<(), MandateError> {
    let n = legs.len();
    if n == 0 || n > MAX_LEGS {
        return Err(MandateError::LegsInvalid);
    }
    if legs.iter().any(|leg| leg.amount < 1) {
        return Err(MandateError::LegsInvalid);
    }
    // Every unit that reaches the fee address must be the fee, so the cap
    // on it means something.
    if legs
        .iter()
        .any(|leg| leg.destination.address() == *fee_address)
    {
        return Err(MandateError::LegsInvalid);
    }
    for (i, a) in legs.iter().enumerate() {
        if legs.iter().skip(i + 1).any(|b| same_destination(&a, &b)) {
            return Err(MandateError::LegsInvalid);
        }
    }
    // Compared against what is left of the total, so the running sum can
    // never pass the total and never overflow.
    let mut running: i128 = 0;
    for leg in legs.iter() {
        let left = invariant(total.checked_sub(running))?;
        if leg.amount > left {
            return Err(MandateError::LegsSumMismatch);
        }
        running = invariant(running.checked_add(leg.amount))?;
    }
    if running != total {
        return Err(MandateError::LegsSumMismatch);
    }
    Ok(())
}

/// Binds every leg to the recipient: on the allowlist, attested, or both,
/// by the mode. Each leg is checked in full before the next one starts.
/// Returns the earliest `valid_until` among the attestations, if any.
fn check_recipients(
    env: &Env,
    mode: u32,
    policy: &Policy,
    req: &PayoutRequest,
    now: u64,
) -> Result<Option<u64>, MandateError> {
    let allowlist = mode == MODE_ALLOWLIST || mode == MODE_BOTH;
    let attested = mode == MODE_ATTESTED || mode == MODE_BOTH;
    if !attested && !req.attestations.is_empty() {
        return Err(MandateError::AttestationInvalid);
    }
    if attested && req.attestations.len() != req.legs.len() {
        return Err(MandateError::AttestationInvalid);
    }
    let attester = if attested {
        Some(attest::attester_key(policy)?)
    } else {
        None
    };
    let mut earliest: Option<u64> = None;
    for (i, leg) in req.legs.iter().enumerate() {
        if allowlist && !storage::is_allowed(env, &req.recipient_ref, &leg.destination) {
            return Err(MandateError::RecipientNotAllowed);
        }
        if let Some(key) = &attester {
            let index = invariant(u32::try_from(i).ok())?;
            let att = invariant(req.attestations.get(index))?;
            let valid_until = attest::check_leg(
                env,
                key,
                policy,
                &req.recipient_ref,
                &leg.destination,
                &att,
                now,
            )?;
            earliest = Some(earliest.map_or(valid_until, |e| e.min(valid_until)));
        }
    }
    Ok(earliest)
}

// Approval and rejection.

/// The number of approvals given by addresses that are approvers now. An
/// approval from someone since removed no longer counts.
fn live_approvals(roles: &Roles, record: &PayoutRecord) -> Result<u32, MandateError> {
    let n = record
        .approvals
        .iter()
        .filter(|a| roles.approvers.contains(a))
        .count();
    invariant(u32::try_from(n).ok())
}

pub fn approve(env: &Env, caller: &Address, payout_id: &BytesN<32>) -> Result<u32, MandateError> {
    caller.require_auth();
    let roles = storage::roles(env)?;
    if !roles.approvers.contains(caller) {
        return Err(MandateError::Unauthorized);
    }
    let now = env.ledger().timestamp();
    ensure_active(env, now)?;
    let mut record = open_record(env, payout_id)?;
    if record.status != STATUS_PENDING {
        return Err(MandateError::PayoutWrongStatus);
    }
    if now >= record.expires_at {
        return Err(MandateError::PayoutLapsed);
    }
    // A payout queued by an agent that was since removed or rotated out
    // stops here, so taking a compromised key away stops its queue too.
    if !same_agent(&roles, &record) {
        return Err(MandateError::AgentRemoved);
    }
    let mut live = live_approvals(&roles, &record)?;
    let repeat = record.approvals.contains(caller);
    // A repeat is refused only while it would change nothing. Once the
    // requirement is met, for example after the principal lowered it, a
    // repeat goes on to the transition so the payout never gets stuck.
    if repeat && live < roles.approvals_required {
        return Err(MandateError::AlreadyApproved);
    }
    if !repeat {
        record.approvals.push_back(caller.clone());
        live = invariant(live.checked_add(1))?;
    }
    let completes = live >= roles.approvals_required;
    let mut totals = storage::totals(env)?;
    if completes {
        let charge = charged(record.total, record.fee)?;
        release(&mut totals, STATUS_PENDING, charge)?;
        totals.committed = invariant(totals.committed.checked_add(charge))?;
        record.status = STATUS_APPROVED;
    }

    storage::update_record(env, &record);
    if !repeat {
        PayoutApprovalAdded {
            payout_id: payout_id.clone(),
            approver: caller.clone(),
            approvals: live,
            required: roles.approvals_required,
        }
        .publish(env);
    }
    if completes {
        storage::set_totals(env, &totals);
        PayoutApproved {
            payout_id: payout_id.clone(),
        }
        .publish(env);
    }
    Ok(record.status)
}

/// One rejection is a veto. Allowed while paused and after expiry, since
/// it only ever releases a reservation.
pub fn reject(env: &Env, caller: &Address, payout_id: &BytesN<32>) -> Result<(), MandateError> {
    caller.require_auth();
    let roles = storage::roles(env)?;
    if !roles.approvers.contains(caller) && *caller != storage::principal(env)? {
        return Err(MandateError::Unauthorized);
    }
    ensure_not_revoked(env)?;
    let record = open_record(env, payout_id)?;
    if record.status != STATUS_PENDING {
        return Err(MandateError::PayoutWrongStatus);
    }
    let mut totals = storage::totals(env)?;
    release(
        &mut totals,
        STATUS_PENDING,
        charged(record.total, record.fee)?,
    )?;

    storage::put_tombstone(env, payout_id, STATUS_REJECTED);
    storage::set_totals(env, &totals);
    PayoutRejected {
        payout_id: payout_id.clone(),
        by: caller.clone(),
    }
    .publish(env);
    Ok(())
}

// Cancels.

pub fn cancel(env: &Env, caller: &Address, payout_id: &BytesN<32>) -> Result<(), MandateError> {
    caller.require_auth();
    ensure_not_revoked(env)?;
    cancel_item(env, caller, payout_id)
}

/// Every item is cancelled or refused on its own; a refused item never
/// reverts the batch and emits nothing.
pub fn cancel_batch(
    env: &Env,
    caller: &Address,
    ids: &Vec<BytesN<32>>,
) -> Result<Vec<ItemResult>, MandateError> {
    caller.require_auth();
    ensure_not_revoked(env)?;
    if ids.is_empty() || ids.len() > MAX_BATCH {
        return Err(MandateError::BatchInvalid);
    }
    let mut out = Vec::new(env);
    for id in ids.iter() {
        let result = match cancel_item(env, caller, &id) {
            Ok(()) => ItemResult {
                payout_id: id,
                code: ITEM_OK,
                status: Some(STATUS_CANCELLED),
            },
            Err(e) => ItemResult {
                payout_id: id,
                code: e as u32,
                status: None,
            },
        };
        out.push_back(result);
    }
    Ok(out)
}

/// The cancel checks from the lookup onward, then the effects. Who may
/// cancel depends on the payout, so the role decision comes after the
/// lookup here and nowhere else.
fn cancel_item(env: &Env, caller: &Address, payout_id: &BytesN<32>) -> Result<(), MandateError> {
    let record = open_record(env, payout_id)?;
    if record.status != STATUS_PENDING && record.status != STATUS_APPROVED {
        return Err(MandateError::PayoutWrongStatus);
    }
    let reason = cancel_reason(env, caller, &record)?;
    let mut totals = storage::totals(env)?;
    release(
        &mut totals,
        record.status,
        charged(record.total, record.fee)?,
    )?;

    storage::put_tombstone(env, payout_id, STATUS_CANCELLED);
    storage::set_totals(env, &totals);
    PayoutCancelled {
        payout_id: payout_id.clone(),
        by: caller.clone(),
        reason,
    }
    .publish(env);
    Ok(())
}

/// The principal always may. The payout's own agent may while it is still
/// the same agent, so one agent can never cancel what people approved for
/// another, and a removed agent added back cannot touch its old queue.
/// Anyone may once the payout itself lapsed; a cancel that rests only on
/// the mandate's expiry waits out the grace, which gives the principal time
/// to extend the mandate before strangers start unwinding it.
fn cancel_reason(env: &Env, caller: &Address, record: &PayoutRecord) -> Result<u32, MandateError> {
    if *caller == storage::principal(env)? {
        return Ok(CANCEL_BY_PRINCIPAL);
    }
    if *caller == record.agent && same_agent(&storage::roles(env)?, record) {
        return Ok(CANCEL_BY_AGENT);
    }
    let now = env.ledger().timestamp();
    if now >= record.expires_at {
        return Ok(CANCEL_LAPSED);
    }
    let expires_at = storage::limits(env)?.expires_at;
    // Once expired, now >= expires_at, so the subtraction cannot underflow.
    if derived::is_expired(now, expires_at)
        && invariant(now.checked_sub(expires_at))? >= SWEEP_GRACE_SECONDS
    {
        return Ok(CANCEL_LAPSED);
    }
    Err(MandateError::Unauthorized)
}
