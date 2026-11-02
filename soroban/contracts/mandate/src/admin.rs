// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The principal's administration: limits, roles and their gates, the
//! allowlist, the attestation policy, the principal transfer and the fee
//! cap.
//!
//! Every call here takes no caller argument and requires the principal's
//! authorization, except `accept_principal`, which requires the proposed
//! principal's. Each one applies in full or not at all: every check runs
//! before anything is written or emitted.
//!
//! The role rules this enforces on every change:
//!
//! - the principal is never an agent, an executor or a recovery approver;
//! - agents, approvers and executors are pairwise disjoint, and recovery
//!   approvers are neither agents nor executors;
//! - the fee address holds no role at all;
//! - the attester's key is the key of no role account;
//! - `approvals_required` and `recovery_threshold` always fit their sets,
//!   following them when the first member is added or the last removed.
//!
//! Role lists keep the order members were added in: a member is appended on
//! add and taken out on removal without moving the others, so `config()`
//! and the events always report a set the same way.

use crate::events::{
    ApprovalsRequiredSet, AttestationPolicySet, FeeCapLowered, LimitsUpdated,
    PrincipalProposalCancelled, PrincipalProposed, PrincipalTransferred, RecipientAllowed,
    RecipientDisallowed, RecoveryThresholdSet, RoleAdded, RoleRemoved,
};
use crate::lifecycle::authorized_principal;
use crate::payout::{ensure_not_revoked, release_agent, MAX_BATCH};
use crate::storage::{self, Agent, Policy, Roles};
use crate::types::{
    AllowEntry, ROLE_AGENT, ROLE_APPROVER, ROLE_EXECUTOR, ROLE_GUARDIAN, ROLE_RECOVERY,
};
use crate::{derived, UjiraMandate, UjiraMandateArgs, UjiraMandateClient};
use mandate_config::{
    Limits, MAX_AGENTS, MAX_APPROVERS, MAX_EXECUTORS, MAX_GUARDIANS, MAX_RECOVERY, MODE_ALLOWLIST,
    MODE_ATTESTED,
};
use mandate_errors::MandateError;
use soroban_sdk::{contractimpl, Address, Bytes, BytesN, Env, Vec};

#[contractimpl]
impl UjiraMandate {
    /// Replaces all six limits at once.
    pub fn update_limits(env: Env, limits: Limits) -> Result<(), MandateError> {
        storage::extend_instance(&env);
        update_limits(&env, &limits)
    }

    /// Adds `account` to a role set: 1 agent, 2 approver, 3 guardian,
    /// 4 executor, 5 recovery approver.
    pub fn add_role(env: Env, role: u32, account: Address) -> Result<(), MandateError> {
        storage::extend_instance(&env);
        add_role(&env, role, &account)
    }

    /// Takes `account` out of a role set.
    pub fn remove_role(env: Env, role: u32, account: Address) -> Result<(), MandateError> {
        storage::extend_instance(&env);
        remove_role(&env, role, &account)
    }

    /// How many distinct approvers a payout needs.
    pub fn set_approvals_required(env: Env, n: u32) -> Result<(), MandateError> {
        storage::extend_instance(&env);
        set_approvals_required(&env, n)
    }

    /// How many recovery approvers must vote for a recovery action.
    pub fn set_recovery_threshold(env: Env, n: u32) -> Result<(), MandateError> {
        storage::extend_instance(&env);
        set_recovery_threshold(&env, n)
    }

    /// Binds up to ten destinations, tag included, to their recipients.
    pub fn allow_recipients(env: Env, entries: Vec<AllowEntry>) -> Result<(), MandateError> {
        storage::extend_instance(&env);
        allow_recipients(&env, &entries)
    }

    /// Unbinds up to ten destinations. Queued payouts to them stop at
    /// execute.
    pub fn disallow_recipients(env: Env, entries: Vec<AllowEntry>) -> Result<(), MandateError> {
        storage::extend_instance(&env);
        disallow_recipients(&env, &entries)
    }

    /// Replaces the attester key and the country policy, which invalidates
    /// every attestation issued before.
    pub fn set_attestation_policy(
        env: Env,
        attester: Bytes,
        country_policy_hash: BytesN<32>,
    ) -> Result<(), MandateError> {
        storage::extend_instance(&env);
        set_attestation_policy(&env, &attester, &country_policy_hash)
    }

    /// Names the address that may take over as principal.
    pub fn propose_principal(env: Env, proposed: Address) -> Result<(), MandateError> {
        storage::extend_instance(&env);
        propose_principal(&env, &proposed)
    }

    /// Takes over as principal, as the proposed address.
    pub fn accept_principal(env: Env) -> Result<(), MandateError> {
        storage::extend_instance(&env);
        accept_principal(&env)
    }

    /// Withdraws a principal proposal.
    pub fn cancel_principal_proposal(env: Env) -> Result<(), MandateError> {
        storage::extend_instance(&env);
        cancel_principal_proposal(&env)
    }

    /// Lowers the fee cap. It can never go back up.
    pub fn lower_fee_cap(env: Env, new_bps: u32) -> Result<(), MandateError> {
        storage::extend_instance(&env);
        lower_fee_cap(&env, new_bps)
    }
}

fn tenant_tag(env: &Env) -> Result<BytesN<32>, MandateError> {
    Ok(storage::fixed(env)?.tenant_tag)
}

// Limits.

pub fn update_limits(env: &Env, limits: &Limits) -> Result<(), MandateError> {
    authorized_principal(env)?;
    ensure_not_revoked(env)?;
    let now = env.ledger().timestamp();
    mandate_config::check_limits(limits, now)?;
    // With no approvers nothing may need an approval, whichever of the
    // threshold and the cap is changing. With approvers this always holds.
    let roles = storage::roles(env)?;
    mandate_config::check_approvals_required(
        roles.approvals_required,
        roles.approvers.len(),
        limits,
    )?;
    let totals = storage::totals(env)?;
    let reserved = totals
        .spent
        .checked_add(totals.pending)
        .and_then(|s| s.checked_add(totals.committed))
        .ok_or(MandateError::InternalInvariant)?;
    if limits.total_budget < reserved {
        return Err(MandateError::BudgetBelowReserved);
    }
    // Extending an expired mandate makes it active again. While it was
    // expired the whole balance was free, so the reservations may no longer
    // be covered; reactivating then would leave approved payouts that can
    // only fail at execute.
    if derived::is_expired(now, storage::limits(env)?.expires_at) {
        let open = totals
            .pending
            .checked_add(totals.committed)
            .ok_or(MandateError::InternalInvariant)?;
        if derived::balance(env, &storage::fixed(env)?.asset) < open {
            return Err(MandateError::InsufficientFunds);
        }
    }

    storage::set_limits(env, limits);
    LimitsUpdated {
        tenant_tag: tenant_tag(env)?,
        total_budget: limits.total_budget,
        per_payment_cap: limits.per_payment_cap,
        per_recipient_daily_cap: limits.per_recipient_daily_cap,
        approval_threshold: limits.approval_threshold,
        payout_ttl_seconds: limits.payout_ttl_seconds,
        expires_at: limits.expires_at,
    }
    .publish(env);
    Ok(())
}

// Roles.

/// Whether `account` is in the set `role`; `CONFIG_INVALID` for a role code
/// that names no set.
fn holds(roles: &Roles, role: u32, account: &Address) -> Result<bool, MandateError> {
    Ok(match role {
        ROLE_AGENT => roles.agent(account).is_some(),
        ROLE_APPROVER => roles.approvers.contains(account),
        ROLE_GUARDIAN => roles.guardians.contains(account),
        ROLE_EXECUTOR => roles.executors.contains(account),
        ROLE_RECOVERY => roles.recovery.contains(account),
        _ => return Err(MandateError::ConfigInvalid),
    })
}

/// The size of a known set and its bound.
fn size_and_bound(roles: &Roles, role: u32) -> Result<(u32, u32), MandateError> {
    Ok(match role {
        ROLE_AGENT => (roles.agents.len(), MAX_AGENTS),
        ROLE_APPROVER => (roles.approvers.len(), MAX_APPROVERS),
        ROLE_GUARDIAN => (roles.guardians.len(), MAX_GUARDIANS),
        ROLE_EXECUTOR => (roles.executors.len(), MAX_EXECUTORS),
        ROLE_RECOVERY => (roles.recovery.len(), MAX_RECOVERY),
        _ => return Err(MandateError::ConfigInvalid),
    })
}

/// Whether putting `account` into the set `role` would break role
/// separation. Every overlap not refused here is one the rules allow: the
/// principal may approve or guard, a guardian may hold any other role, and
/// a recovery approver may approve or guard. Also the check on the new
/// agent of a recovery rotation, which must meet the same rules.
pub(crate) fn breaks_separation(
    roles: &Roles,
    principal: &Address,
    fee_address: &Address,
    role: u32,
    account: &Address,
) -> bool {
    let is_principal = account == principal;
    let agent = roles.agent(account).is_some();
    let approver = roles.approvers.contains(account);
    let executor = roles.executors.contains(account);
    let recovery = roles.recovery.contains(account);
    let refused = match role {
        // The principal's key is never used for routine spending, an agent
        // never approves its own request, and an agent already executes.
        ROLE_AGENT => is_principal || approver || recovery || executor,
        ROLE_APPROVER => agent || executor,
        ROLE_GUARDIAN => false,
        // Whoever submits is never also the approval gate.
        ROLE_EXECUTOR => is_principal || approver || recovery || agent,
        // A spending or submitting key never votes to rotate or revoke, and
        // the principal's seat would be a vote lost exactly when its key is.
        ROLE_RECOVERY => is_principal || agent || executor,
        _ => false,
    };
    // The fee address is used for nothing but receiving fees.
    refused || account == fee_address
}

/// Whether the attester key, if one is set, is `account`'s own ed25519
/// key. A contract address has no such key and never matches.
pub(crate) fn is_attester_key_of(env: &Env, policy: &Policy, account: &Address) -> bool {
    match &policy.attester {
        Some(key) => mandate_config::account_key_is(env, account, key),
        None => false,
    }
}

/// Appends an agent under the next `agent_epoch`. The counter goes up
/// first, so an agent added after creation, or added back after a removal,
/// holds a value no payout queued before it carries.
pub(crate) fn push_agent(roles: &mut Roles, account: &Address) -> Result<(), MandateError> {
    roles.agent_epoch = roles
        .agent_epoch
        .checked_add(1)
        .ok_or(MandateError::InternalInvariant)?;
    roles.agents.push_back(Agent {
        account: account.clone(),
        epoch: roles.agent_epoch,
        pending: 0,
        committed: 0,
    });
    Ok(())
}

/// Votes are kept per recovery epoch, so moving it on discards every vote
/// cast before. Called once per call that changes the agents, the recovery
/// set or its threshold, or runs a recovery action.
pub(crate) fn bump_recovery_epoch(roles: &mut Roles) -> Result<(), MandateError> {
    roles.recovery_epoch = roles
        .recovery_epoch
        .checked_add(1)
        .ok_or(MandateError::InternalInvariant)?;
    Ok(())
}

/// Takes `account` out of `list` without reordering the rest.
fn remove_member(list: &mut Vec<Address>, account: &Address) -> Result<(), MandateError> {
    let i = list
        .first_index_of(account)
        .ok_or(MandateError::InternalInvariant)?;
    list.remove(i).ok_or(MandateError::InternalInvariant)
}

pub fn add_role(env: &Env, role: u32, account: &Address) -> Result<(), MandateError> {
    let principal = authorized_principal(env)?;
    ensure_not_revoked(env)?;
    let mut roles = storage::roles(env)?;
    if holds(&roles, role, account)? {
        return Err(MandateError::ConfigInvalid);
    }
    let fixed = storage::fixed(env)?;
    if breaks_separation(&roles, &principal, &fixed.fee_address, role, account) {
        return Err(MandateError::RoleConflict);
    }
    if is_attester_key_of(env, &storage::policy(env)?, account) {
        return Err(MandateError::RoleConflict);
    }
    let (size, bound) = size_and_bound(&roles, role)?;
    if size >= bound {
        return Err(MandateError::RoleSetFull);
    }

    // A first approver or recovery approver brings its gate up to 1 in the
    // same call, so the gate always fits the set.
    let mut approvals_set = false;
    let mut threshold_set = false;
    match role {
        ROLE_AGENT => {
            push_agent(&mut roles, account)?;
            bump_recovery_epoch(&mut roles)?;
        }
        ROLE_APPROVER => {
            approvals_set = roles.approvers.is_empty();
            roles.approvers.push_back(account.clone());
            if approvals_set {
                roles.approvals_required = 1;
            }
        }
        ROLE_GUARDIAN => roles.guardians.push_back(account.clone()),
        ROLE_EXECUTOR => roles.executors.push_back(account.clone()),
        ROLE_RECOVERY => {
            threshold_set = roles.recovery.is_empty();
            roles.recovery.push_back(account.clone());
            if threshold_set {
                roles.recovery_threshold = 1;
            }
            bump_recovery_epoch(&mut roles)?;
        }
        _ => return Err(MandateError::ConfigInvalid),
    }
    storage::set_roles(env, &roles);

    RoleAdded {
        tenant_tag: fixed.tenant_tag.clone(),
        role,
        account: account.clone(),
    }
    .publish(env);
    if approvals_set {
        ApprovalsRequiredSet {
            tenant_tag: fixed.tenant_tag.clone(),
            required: roles.approvals_required,
        }
        .publish(env);
    }
    if threshold_set {
        RecoveryThresholdSet {
            tenant_tag: fixed.tenant_tag,
            required: roles.recovery_threshold,
            recovery_epoch: roles.recovery_epoch,
        }
        .publish(env);
    }
    Ok(())
}

pub fn remove_role(env: &Env, role: u32, account: &Address) -> Result<(), MandateError> {
    authorized_principal(env)?;
    ensure_not_revoked(env)?;
    let mut roles = storage::roles(env)?;
    if !holds(&roles, role, account)? {
        return Err(MandateError::ConfigInvalid);
    }
    // The approval gate must stay reachable: with several approvers the
    // requirement must still fit the ones left; the last one may go only if
    // no payout can need an approval afterwards.
    if role == ROLE_APPROVER {
        let left = roles
            .approvers
            .len()
            .checked_sub(1)
            .ok_or(MandateError::InternalInvariant)?;
        if left > 0 && left < roles.approvals_required {
            return Err(MandateError::ConfigInvalid);
        }
        let limits = storage::limits(env)?;
        if left == 0 && limits.approval_threshold <= limits.per_payment_cap {
            return Err(MandateError::ConfigInvalid);
        }
    }
    // The same for the recovery threshold among several members. The last
    // member may always go, since no payout ever waits on a recovery vote.
    if role == ROLE_RECOVERY {
        let left = roles
            .recovery
            .len()
            .checked_sub(1)
            .ok_or(MandateError::InternalInvariant)?;
        if left > 0 && left < roles.recovery_threshold {
            return Err(MandateError::ConfigInvalid);
        }
    }

    let mut approvals_set = false;
    let mut threshold_set = false;
    match role {
        ROLE_AGENT => {
            let i = roles
                .agents
                .iter()
                .position(|a| a.account == *account)
                .and_then(|i| u32::try_from(i).ok())
                .ok_or(MandateError::InternalInvariant)?;
            let gone = roles.agents.get(i).ok_or(MandateError::InternalInvariant)?;
            roles
                .agents
                .remove(i)
                .ok_or(MandateError::InternalInvariant)?;
            release_agent(env, &gone)?;
            bump_recovery_epoch(&mut roles)?;
        }
        ROLE_APPROVER => {
            remove_member(&mut roles.approvers, account)?;
            approvals_set = roles.approvers.is_empty();
            if approvals_set {
                roles.approvals_required = 0;
            }
        }
        ROLE_GUARDIAN => remove_member(&mut roles.guardians, account)?,
        ROLE_EXECUTOR => remove_member(&mut roles.executors, account)?,
        ROLE_RECOVERY => {
            remove_member(&mut roles.recovery, account)?;
            threshold_set = roles.recovery.is_empty();
            if threshold_set {
                roles.recovery_threshold = 0;
            }
            bump_recovery_epoch(&mut roles)?;
        }
        _ => return Err(MandateError::ConfigInvalid),
    }
    storage::set_roles(env, &roles);

    let tenant_tag = tenant_tag(env)?;
    RoleRemoved {
        tenant_tag: tenant_tag.clone(),
        role,
        account: account.clone(),
    }
    .publish(env);
    if approvals_set {
        ApprovalsRequiredSet {
            tenant_tag: tenant_tag.clone(),
            required: 0,
        }
        .publish(env);
    }
    if threshold_set {
        RecoveryThresholdSet {
            tenant_tag,
            required: 0,
            recovery_epoch: roles.recovery_epoch,
        }
        .publish(env);
    }
    Ok(())
}

pub fn set_approvals_required(env: &Env, n: u32) -> Result<(), MandateError> {
    authorized_principal(env)?;
    ensure_not_revoked(env)?;
    let mut roles = storage::roles(env)?;
    mandate_config::check_approvals_required(n, roles.approvers.len(), &storage::limits(env)?)?;
    roles.approvals_required = n;
    storage::set_roles(env, &roles);
    ApprovalsRequiredSet {
        tenant_tag: tenant_tag(env)?,
        required: n,
    }
    .publish(env);
    Ok(())
}

pub fn set_recovery_threshold(env: &Env, n: u32) -> Result<(), MandateError> {
    authorized_principal(env)?;
    ensure_not_revoked(env)?;
    let mut roles = storage::roles(env)?;
    mandate_config::check_recovery_threshold(n, roles.recovery.len())?;
    roles.recovery_threshold = n;
    bump_recovery_epoch(&mut roles)?;
    storage::set_roles(env, &roles);
    RecoveryThresholdSet {
        tenant_tag: tenant_tag(env)?,
        required: n,
        recovery_epoch: roles.recovery_epoch,
    }
    .publish(env);
    Ok(())
}

// The allowlist.

fn same_entry(a: &AllowEntry, b: &AllowEntry) -> bool {
    a.recipient_ref == b.recipient_ref
        && a.destination.address() == b.destination.address()
        && a.destination.id() == b.destination.id()
}

/// The checks both allowlist calls share, in their order: the mode, the
/// batch length, then the fee address on any entry. Stellar has no zero
/// address and a muxed address cannot mark a tag absent while carrying a
/// value, so those two entry rules have nothing to check here. Returns the
/// fixed configuration for the effects.
fn check_entries(env: &Env, entries: &Vec<AllowEntry>) -> Result<storage::Fixed, MandateError> {
    let fixed = storage::fixed(env)?;
    if fixed.recipient_mode == MODE_ATTESTED {
        return Err(MandateError::WrongRecipientMode);
    }
    if entries.is_empty() || entries.len() > MAX_BATCH {
        return Err(MandateError::BatchInvalid);
    }
    if entries
        .iter()
        .any(|e| e.destination.address() == fixed.fee_address)
    {
        return Err(MandateError::ConfigInvalid);
    }
    Ok(fixed)
}

/// True when an entry before position `i` names the same pair, so a pair
/// given twice in one call counts as present for its second copy.
fn repeated_before(entries: &Vec<AllowEntry>, i: usize, entry: &AllowEntry) -> bool {
    entries.iter().take(i).any(|e| same_entry(&e, entry))
}

pub fn allow_recipients(env: &Env, entries: &Vec<AllowEntry>) -> Result<(), MandateError> {
    authorized_principal(env)?;
    ensure_not_revoked(env)?;
    let fixed = check_entries(env, entries)?;
    for (i, e) in entries.iter().enumerate() {
        if storage::is_allowed(env, &e.recipient_ref, &e.destination)
            || repeated_before(entries, i, &e)
        {
            return Err(MandateError::ConfigInvalid);
        }
    }
    for e in entries.iter() {
        storage::set_allowed(env, &e.recipient_ref, &e.destination);
        RecipientAllowed {
            tenant_tag: fixed.tenant_tag.clone(),
            recipient_ref: e.recipient_ref,
            destination: e.destination,
        }
        .publish(env);
    }
    Ok(())
}

pub fn disallow_recipients(env: &Env, entries: &Vec<AllowEntry>) -> Result<(), MandateError> {
    authorized_principal(env)?;
    ensure_not_revoked(env)?;
    let fixed = check_entries(env, entries)?;
    for (i, e) in entries.iter().enumerate() {
        if !storage::is_allowed(env, &e.recipient_ref, &e.destination)
            || repeated_before(entries, i, &e)
        {
            return Err(MandateError::ConfigInvalid);
        }
    }
    for e in entries.iter() {
        storage::remove_allowed(env, &e.recipient_ref, &e.destination);
        RecipientDisallowed {
            tenant_tag: fixed.tenant_tag.clone(),
            recipient_ref: e.recipient_ref,
            destination: e.destination,
        }
        .publish(env);
    }
    Ok(())
}

// The attestation policy.

/// Whether `key` is the ed25519 key of the principal, the fee address or
/// any role member.
fn key_holds_role(
    env: &Env,
    key: &Bytes,
    principal: &Address,
    fee_address: &Address,
    roles: &Roles,
) -> bool {
    let is = |a: &Address| mandate_config::account_key_is(env, a, key);
    is(principal)
        || is(fee_address)
        || roles.agents.iter().any(|a| is(&a.account))
        || roles.approvers.iter().any(|a| is(&a))
        || roles.guardians.iter().any(|a| is(&a))
        || roles.executors.iter().any(|a| is(&a))
        || roles.recovery.iter().any(|a| is(&a))
}

/// A new key or country policy moves the policy epoch on, which stops every
/// payout attested under the old one at execute and every attestation
/// issued under it at request.
pub fn set_attestation_policy(
    env: &Env,
    attester: &Bytes,
    country_policy_hash: &BytesN<32>,
) -> Result<(), MandateError> {
    let principal = authorized_principal(env)?;
    ensure_not_revoked(env)?;
    let fixed = storage::fixed(env)?;
    if fixed.recipient_mode == MODE_ALLOWLIST {
        return Err(MandateError::WrongRecipientMode);
    }
    if !mandate_config::attester_key_well_formed(attester) {
        return Err(MandateError::ConfigInvalid);
    }
    let roles = storage::roles(env)?;
    if key_holds_role(env, attester, &principal, &fixed.fee_address, &roles) {
        return Err(MandateError::RoleConflict);
    }
    let mut policy = storage::policy(env)?;
    policy.attester = Some(attester.clone());
    policy.country_policy_hash = country_policy_hash.clone();
    policy.policy_epoch = policy
        .policy_epoch
        .checked_add(1)
        .ok_or(MandateError::InternalInvariant)?;
    storage::set_policy(env, &policy);
    AttestationPolicySet {
        tenant_tag: fixed.tenant_tag,
        attester: attester.clone(),
        country_policy_hash: country_policy_hash.clone(),
        policy_epoch: policy.policy_epoch,
    }
    .publish(env);
    Ok(())
}

// The principal transfer. Allowed in every state, revoked included, so a
// principal that lost or retired its key can still move to a new address
// and take the funds back there.

/// The roles a principal may never hold: agent, executor or recovery
/// approver.
fn conflicts_as_principal(roles: &Roles, account: &Address) -> bool {
    roles.agent(account).is_some()
        || roles.executors.contains(account)
        || roles.recovery.contains(account)
}

pub fn propose_principal(env: &Env, proposed: &Address) -> Result<(), MandateError> {
    let principal = authorized_principal(env)?;
    let roles = storage::roles(env)?;
    let fixed = storage::fixed(env)?;
    if conflicts_as_principal(&roles, proposed) || *proposed == fixed.fee_address {
        return Err(MandateError::RoleConflict);
    }
    if is_attester_key_of(env, &storage::policy(env)?, proposed) {
        return Err(MandateError::RoleConflict);
    }
    storage::set_proposed_principal(env, Some(proposed));
    PrincipalProposed {
        tenant_tag: fixed.tenant_tag,
        current: principal,
        proposed: proposed.clone(),
    }
    .publish(env);
    Ok(())
}

/// The roles are checked again here, because the proposed address may have
/// gained one, or the attester changed to its key, since the proposal.
pub fn accept_principal(env: &Env) -> Result<(), MandateError> {
    // With no proposal there is no address to authorize.
    let proposed = storage::proposed_principal(env).ok_or(MandateError::Unauthorized)?;
    proposed.require_auth();
    if conflicts_as_principal(&storage::roles(env)?, &proposed) {
        return Err(MandateError::RoleConflict);
    }
    if is_attester_key_of(env, &storage::policy(env)?, &proposed) {
        return Err(MandateError::RoleConflict);
    }
    let previous = storage::principal(env)?;
    storage::set_principal(env, &proposed);
    storage::set_proposed_principal(env, None);
    PrincipalTransferred {
        tenant_tag: tenant_tag(env)?,
        previous,
        current: proposed,
    }
    .publish(env);
    Ok(())
}

pub fn cancel_principal_proposal(env: &Env) -> Result<(), MandateError> {
    let principal = authorized_principal(env)?;
    let proposed = storage::proposed_principal(env).ok_or(MandateError::ConfigInvalid)?;
    storage::set_proposed_principal(env, None);
    PrincipalProposalCancelled {
        tenant_tag: tenant_tag(env)?,
        current: principal,
        proposed,
    }
    .publish(env);
    Ok(())
}

// The fee cap.

/// Allowed while paused and after expiry. Raising the cap needs a new
/// mandate, and nothing else, recovery included, can change it.
pub fn lower_fee_cap(env: &Env, new_bps: u32) -> Result<(), MandateError> {
    authorized_principal(env)?;
    ensure_not_revoked(env)?;
    let mut policy = storage::policy(env)?;
    if new_bps >= policy.max_fee_bps {
        return Err(MandateError::ConfigInvalid);
    }
    policy.max_fee_bps = new_bps;
    storage::set_policy(env, &policy);
    FeeCapLowered {
        tenant_tag: tenant_tag(env)?,
        max_fee_bps: new_bps,
    }
    .publish(env);
    Ok(())
}
