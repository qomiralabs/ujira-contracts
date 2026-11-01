// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The recovery set's two actions, for when the principal's key is lost:
//! revoke the mandate, or rotate one agent out and optionally another in.
//!
//! Each member votes once per action and recovery epoch. When the votes of
//! current members reach `recovery_threshold`, the action runs exactly as
//! the principal's own path would and the epoch moves on. Any change to the
//! agents, the recovery set or its threshold also moves the epoch on, so a
//! vote cast before such a change can never complete an action after it.
//!
//! Recovery can never move funds or widen where they may go. A recovery
//! revoke only freezes the balance for the principal, and a rotation only
//! exchanges one agent for another under the same destinations and limits.
//! Nothing here touches the allowlist, the attestation policy, a limit, the
//! fee cap, the principal or any role set other than the agents.

use crate::admin::{breaks_separation, bump_recovery_epoch, is_attester_key_of, push_agent};
use crate::events::{RecoveryExecuted, RecoveryVoteAdded, RoleAdded, RoleRemoved};
use crate::lifecycle::revoke_effects;
use crate::payout::ensure_not_revoked;
use crate::storage::{self, Roles};
use crate::types::ROLE_AGENT;
use crate::{UjiraMandate, UjiraMandateArgs, UjiraMandateClient};
use mandate_errors::MandateError;
use soroban_sdk::{contractimpl, contracttype, Address, Env, Vec};

/// Recovery action kinds, as the recovery events carry them.
pub const KIND_REVOKE: u32 = 1;
pub const KIND_ROTATE_AGENT: u32 = 2;

/// What a recovery call returns.
pub const VOTE_RECORDED: u32 = 1;
pub const ACTION_RAN: u32 = 2;

/// Votes live in persistent storage, one entry per epoch and action, so the
/// number of open proposals never grows the instance entry. The variant
/// name differs from every key in `storage`, so the two key types can never
/// collide. An entry left behind by an old epoch is never read again and
/// archives on its own.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveryKey {
    /// The members who voted, keyed by recovery epoch, kind, `old` and
    /// `new`.
    RecoveryVotes(u32, u32, Option<Address>, Option<Address>),
}

#[contractimpl]
impl UjiraMandate {
    /// Votes, as the recovery approver `caller`, to revoke the mandate.
    /// Returns 1 when the vote was recorded and 2 when the revoke ran.
    pub fn recovery_revoke(env: Env, caller: Address) -> Result<u32, MandateError> {
        storage::extend_instance(&env);
        recovery_revoke(&env, &caller)
    }

    /// Votes, as the recovery approver `caller`, to take the agent `old`
    /// out and, when `new` is given, put `new` in. Returns 1 when the vote
    /// was recorded and 2 when the rotation ran.
    pub fn recovery_rotate_agent(
        env: Env,
        caller: Address,
        old: Address,
        new: Option<Address>,
    ) -> Result<u32, MandateError> {
        storage::extend_instance(&env);
        recovery_rotate_agent(&env, &caller, &old, new.as_ref())
    }
}

/// The caller's authorization, then membership, then the revoked check, in
/// that order for both actions.
fn member_roles(env: &Env, caller: &Address) -> Result<Roles, MandateError> {
    caller.require_auth();
    let roles = storage::roles(env)?;
    if !roles.recovery.contains(caller) {
        return Err(MandateError::Unauthorized);
    }
    ensure_not_revoked(env)?;
    Ok(roles)
}

/// A vote that passed its checks but is not yet stored.
struct Ballot {
    key: RecoveryKey,
    voters: Vec<Address>,
    votes: u32,
}

/// Refuses a second vote by the same member for the same action in this
/// epoch, then counts the votes with this one. Only current members count;
/// every change to the set moves the epoch on, so in practice every voter
/// in this epoch is one.
fn ballot(
    env: &Env,
    roles: &Roles,
    caller: &Address,
    kind: u32,
    old: Option<&Address>,
    new: Option<&Address>,
) -> Result<Ballot, MandateError> {
    let key = RecoveryKey::RecoveryVotes(roles.recovery_epoch, kind, old.cloned(), new.cloned());
    let mut voters: Vec<Address> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env));
    if voters.contains(caller) {
        return Err(MandateError::AlreadyApproved);
    }
    voters.push_back(caller.clone());
    let counted = voters.iter().filter(|v| roles.recovery.contains(v)).count();
    let votes = u32::try_from(counted).map_err(|_| MandateError::InternalInvariant)?;
    Ok(Ballot { key, voters, votes })
}

/// Stores the vote, or clears the entry when the action is about to run,
/// and emits `recovery_vote_added` with the epoch the vote was cast in.
/// Returns whether the action runs now. A threshold of 0 with a member in
/// the set breaks the rules, so it is never read as "no votes needed".
fn cast(
    env: &Env,
    roles: &Roles,
    b: &Ballot,
    kind: u32,
    old: Option<&Address>,
    new: Option<&Address>,
) -> Result<bool, MandateError> {
    if roles.recovery_threshold == 0 {
        return Err(MandateError::InternalInvariant);
    }
    let runs = b.votes >= roles.recovery_threshold;
    if runs {
        env.storage().persistent().remove(&b.key);
    } else {
        env.storage().persistent().set(&b.key, &b.voters);
    }
    RecoveryVoteAdded {
        tenant_tag: storage::fixed(env)?.tenant_tag,
        kind,
        old: old.cloned(),
        new: new.cloned(),
        votes: b.votes,
        required: roles.recovery_threshold,
        recovery_epoch: roles.recovery_epoch,
    }
    .publish(env);
    Ok(runs)
}

fn executed(
    env: &Env,
    kind: u32,
    old: Option<&Address>,
    new: Option<&Address>,
    by: &Address,
) -> Result<(), MandateError> {
    RecoveryExecuted {
        tenant_tag: storage::fixed(env)?.tenant_tag,
        kind,
        old: old.cloned(),
        new: new.cloned(),
        by: by.clone(),
    }
    .publish(env);
    Ok(())
}

pub fn recovery_revoke(env: &Env, caller: &Address) -> Result<u32, MandateError> {
    let mut roles = member_roles(env, caller)?;
    let b = ballot(env, &roles, caller, KIND_REVOKE, None, None)?;
    if !cast(env, &roles, &b, KIND_REVOKE, None, None)? {
        return Ok(VOTE_RECORDED);
    }
    // The member whose vote completed the action is who revoked.
    revoke_effects(env, caller)?;
    bump_recovery_epoch(&mut roles)?;
    storage::set_roles(env, &roles);
    executed(env, KIND_REVOKE, None, None, caller)?;
    Ok(ACTION_RAN)
}

pub fn recovery_rotate_agent(
    env: &Env,
    caller: &Address,
    old: &Address,
    new: Option<&Address>,
) -> Result<u32, MandateError> {
    let mut roles = member_roles(env, caller)?;
    // One agent leaves as one joins, so the agent bound holds without a
    // check of its own.
    let at = roles
        .agents
        .iter()
        .position(|a| a.account == *old)
        .and_then(|i| u32::try_from(i).ok())
        .ok_or(MandateError::ConfigInvalid)?;
    if let Some(n) = new {
        if roles.agent(n).is_some() {
            return Err(MandateError::ConfigInvalid);
        }
        // The new agent meets the same separation as one the principal
        // adds, and its key is not the attester's.
        let fixed = storage::fixed(env)?;
        let principal = storage::principal(env)?;
        if breaks_separation(&roles, &principal, &fixed.fee_address, ROLE_AGENT, n) {
            return Err(MandateError::RoleConflict);
        }
        if is_attester_key_of(env, &storage::policy(env)?, n) {
            return Err(MandateError::RoleConflict);
        }
    }
    let b = ballot(env, &roles, caller, KIND_ROTATE_AGENT, Some(old), new)?;
    if !cast(env, &roles, &b, KIND_ROTATE_AGENT, Some(old), new)? {
        return Ok(VOTE_RECORDED);
    }

    // The same effects as the principal's remove_role and add_role, in one
    // epoch step.
    roles
        .agents
        .remove(at)
        .ok_or(MandateError::InternalInvariant)?;
    if let Some(n) = new {
        push_agent(&mut roles, n)?;
    }
    bump_recovery_epoch(&mut roles)?;
    storage::set_roles(env, &roles);
    let tenant_tag = storage::fixed(env)?.tenant_tag;
    RoleRemoved {
        tenant_tag: tenant_tag.clone(),
        role: ROLE_AGENT,
        account: old.clone(),
    }
    .publish(env);
    if let Some(n) = new {
        RoleAdded {
            tenant_tag,
            role: ROLE_AGENT,
            account: n.clone(),
        }
        .publish(env);
    }
    executed(env, KIND_ROTATE_AGENT, Some(old), new, caller)?;
    Ok(ACTION_RAN)
}
