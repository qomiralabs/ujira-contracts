// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Recovery votes and actions: who may vote, votes per action and epoch,
//! the action running exactly as the principal's path would, and recovery
//! never moving funds or changing anything beyond the agents and `revoked`.

extern crate std;

use super::{code, Fx, FUNDED, NOW};
use crate::events::{RecoveryExecuted, RecoveryVoteAdded, Revoked, RoleAdded, RoleRemoved};
use crate::recovery::{ACTION_RAN, KIND_REVOKE, KIND_ROTATE_AGENT, VOTE_RECORDED};
use crate::types::{ROLE_AGENT, ROLE_GUARDIAN};
use crate::{Limits, MandateError, UjiraMandateClient};
use soroban_sdk::{
    testutils::Events as _, token::TokenClient, Address, Bytes, BytesN, Event as _, Vec,
};

/// Two recovery approvers with a threshold of 2, and two agents.
fn two_of_two(fx: &Fx) -> (UjiraMandateClient<'_>, Address, Address) {
    let r2 = fx.account(16);
    let a2 = fx.account(12);
    let mut config = fx.config();
    config.recovery = fx.list(&[&fx.recovery, &r2]);
    config.recovery_threshold = 2;
    config.agents = fx.list(&[&fx.agent, &a2]);
    (fx.ready_with(&config), r2, a2)
}

/// Rotating an agent out releases what its open payouts reserve, in the
/// call that completes the rotation, exactly as a removal by the principal.
#[test]
fn rotate_releases_the_old_agents_reservations() {
    let fx = Fx::new();
    let (m, r2, a2) = two_of_two(&fx);
    m.request_payout(&fx.agent, &fx.req(1, 500));
    m.request_payout(&fx.agent, &fx.req(2, 100));
    m.request_payout(&a2, &fx.req(3, 200));
    let open = (m.state().pending, m.state().committed);
    assert_eq!(open, (500, 300));
    assert_eq!(
        m.recovery_rotate_agent(&fx.recovery, &fx.agent, &None),
        VOTE_RECORDED
    );
    // A vote alone releases nothing.
    assert_eq!((m.state().pending, m.state().committed), open);
    assert_eq!(m.recovery_rotate_agent(&r2, &fx.agent, &None), ACTION_RAN);
    let st = m.state();
    assert_eq!((st.pending, st.committed), (0, 200));
    assert_eq!(st.free, FUNDED - 200);
    // The queue is dead, and a later cancel releases nothing a second time.
    assert_eq!(
        code(m.try_execute(&fx.executor, &fx.b32(2))),
        MandateError::AgentRemoved
    );
    m.cancel(&fx.principal, &fx.b32(2));
    assert_eq!(m.state().committed, 200);
    m.cancel(&fx.principal, &fx.b32(3));
    let st = m.state();
    assert_eq!((st.spent, st.pending, st.committed), (0, 0, 0));
}

/// Everything recovery must never change.
#[derive(Debug, PartialEq)]
struct Untouchable {
    principal: Address,
    proposed: Option<Address>,
    approvers: Vec<Address>,
    approvals_required: u32,
    guardians: Vec<Address>,
    executors: Vec<Address>,
    recovery: Vec<Address>,
    recovery_threshold: u32,
    limits: Limits,
    attester: Option<Bytes>,
    country_policy_hash: BytesN<32>,
    max_fee_bps: u32,
    fee_address: Address,
    allowed: bool,
    spent: i128,
    /// The mandate's own balance. That no other balance moved is shown by
    /// the asset emitting no event at all.
    balance: i128,
}

fn untouchable(fx: &Fx, m: &UjiraMandateClient) -> Untouchable {
    let c = m.config();
    Untouchable {
        principal: c.principal.clone(),
        proposed: c.proposed_principal,
        approvers: c.approvers,
        approvals_required: c.approvals_required,
        guardians: c.guardians,
        executors: c.executors,
        recovery: c.recovery,
        recovery_threshold: c.recovery_threshold,
        limits: c.limits,
        attester: c.attester,
        country_policy_hash: c.country_policy_hash,
        max_fee_bps: c.max_fee_bps,
        fee_address: c.fee_address.clone(),
        allowed: m.is_allowed(&fx.bob(), fx.bob_wallet()),
        spent: m.state().spent,
        balance: TokenClient::new(&fx.env, &fx.asset).balance(&m.address),
    }
}

fn no_token_events(fx: &Fx) {
    assert!(fx
        .env
        .events()
        .all()
        .filter_by_contract(&fx.asset)
        .events()
        .is_empty());
}

#[test]
fn recovery_actions_preserve_invariant_i11() {
    let fx = Fx::new();
    let (m, r2, a2) = two_of_two(&fx);
    m.request_payout(&fx.agent, &fx.req(1, 100));
    let before = untouchable(&fx, &m);
    let a3 = fx.account(13);

    // Rotation with a new agent.
    assert_eq!(
        m.recovery_rotate_agent(&fx.recovery, &fx.agent, &Some(a3.clone())),
        VOTE_RECORDED
    );
    no_token_events(&fx);
    assert_eq!(untouchable(&fx, &m), before);
    assert_eq!(
        m.recovery_rotate_agent(&r2, &fx.agent, &Some(a3.clone())),
        ACTION_RAN
    );
    no_token_events(&fx);
    assert_eq!(untouchable(&fx, &m), before);
    assert_eq!(m.config().agents, fx.list(&[&a2, &a3]));

    // Rotation without one.
    m.recovery_rotate_agent(&fx.recovery, &a2, &None);
    assert_eq!(m.recovery_rotate_agent(&r2, &a2, &None), ACTION_RAN);
    no_token_events(&fx);
    assert_eq!(untouchable(&fx, &m), before);
    assert_eq!(m.config().agents, fx.list(&[&a3]));

    // Revoke: frozen in place, nothing moved.
    m.recovery_revoke(&fx.recovery);
    assert_eq!(m.recovery_revoke(&r2), ACTION_RAN);
    no_token_events(&fx);
    assert_eq!(untouchable(&fx, &m), before);
    let st = m.state();
    assert!(st.revoked);
    assert_eq!((st.balance, st.pending, st.committed), (FUNDED, 0, 0));
}

#[test]
fn recovery_revoke_runs_at_the_threshold_like_the_principals_revoke() {
    let fx = Fx::new();
    let (m, r2, _) = two_of_two(&fx);
    let tenant = fx.b32(0x7e);
    m.request_payout(&fx.agent, &fx.req(1, 500));
    assert_eq!(m.recovery_revoke(&fx.recovery), VOTE_RECORDED);
    assert_eq!(
        fx.events(&m.address),
        [RecoveryVoteAdded {
            tenant_tag: tenant.clone(),
            kind: KIND_REVOKE,
            old: None,
            new: None,
            votes: 1,
            required: 2,
            recovery_epoch: 0,
        }
        .to_xdr(&fx.env, &m.address)]
    );
    assert!(!m.state().revoked);
    assert_eq!(
        code(m.try_recovery_revoke(&fx.recovery)),
        MandateError::AlreadyApproved
    );
    assert_eq!(m.recovery_revoke(&r2), ACTION_RAN);
    assert_eq!(
        fx.events(&m.address),
        [
            RecoveryVoteAdded {
                tenant_tag: tenant.clone(),
                kind: KIND_REVOKE,
                old: None,
                new: None,
                votes: 2,
                required: 2,
                recovery_epoch: 0,
            }
            .to_xdr(&fx.env, &m.address),
            Revoked {
                tenant_tag: tenant.clone(),
                by: r2.clone(),
                frozen: FUNDED,
            }
            .to_xdr(&fx.env, &m.address),
            RecoveryExecuted {
                tenant_tag: tenant,
                kind: KIND_REVOKE,
                old: None,
                new: None,
                by: r2.clone(),
            }
            .to_xdr(&fx.env, &m.address),
        ]
    );
    let st = m.state();
    assert!(st.revoked);
    assert_eq!((st.pending, st.committed), (0, 0));
    assert_eq!(m.config().recovery_epoch, 1);
    assert_eq!(
        code(m.try_recovery_revoke(&fx.recovery)),
        MandateError::MandateRevoked
    );
    assert_eq!(
        code(m.try_recovery_rotate_agent(&r2, &fx.agent, &None)),
        MandateError::MandateRevoked
    );
}

#[test]
fn rotation_events_follow_the_principals_path() {
    let fx = Fx::new();
    let (m, r2, a2) = two_of_two(&fx);
    let tenant = fx.b32(0x7e);
    let a3 = fx.account(13);
    m.recovery_rotate_agent(&fx.recovery, &fx.agent, &Some(a3.clone()));
    m.recovery_rotate_agent(&r2, &fx.agent, &Some(a3.clone()));
    assert_eq!(
        fx.events(&m.address),
        [
            RecoveryVoteAdded {
                tenant_tag: tenant.clone(),
                kind: KIND_ROTATE_AGENT,
                old: Some(fx.agent.clone()),
                new: Some(a3.clone()),
                votes: 2,
                required: 2,
                recovery_epoch: 0,
            }
            .to_xdr(&fx.env, &m.address),
            RoleRemoved {
                tenant_tag: tenant.clone(),
                role: ROLE_AGENT,
                account: fx.agent.clone(),
            }
            .to_xdr(&fx.env, &m.address),
            RoleAdded {
                tenant_tag: tenant.clone(),
                role: ROLE_AGENT,
                account: a3.clone(),
            }
            .to_xdr(&fx.env, &m.address),
            RecoveryExecuted {
                tenant_tag: tenant,
                kind: KIND_ROTATE_AGENT,
                old: Some(fx.agent.clone()),
                new: Some(a3.clone()),
                by: r2.clone(),
            }
            .to_xdr(&fx.env, &m.address),
        ]
    );
    let c = m.config();
    assert_eq!(c.agents, fx.list(&[&a2, &a3]));
    assert_eq!((c.agent_epoch, c.recovery_epoch), (1, 1));
}

#[test]
fn votes_count_per_action_and_epoch() {
    let fx = Fx::new();
    let (m, r2, a2) = two_of_two(&fx);
    let a3 = fx.account(13);
    let a4 = fx.account(14);
    // Different actions keep different tallies.
    m.recovery_revoke(&fx.recovery);
    assert_eq!(
        m.recovery_rotate_agent(&fx.recovery, &fx.agent, &Some(a3.clone())),
        VOTE_RECORDED
    );
    assert_eq!(
        m.recovery_rotate_agent(&r2, &fx.agent, &Some(a4.clone())),
        VOTE_RECORDED
    );
    assert_eq!(
        m.recovery_rotate_agent(&r2, &a2, &Some(a3.clone())),
        VOTE_RECORDED
    );
    // Any change to the agents moves the epoch on and discards every vote.
    m.add_role(&ROLE_AGENT, &fx.account(15));
    assert_eq!(m.recovery_revoke(&r2), VOTE_RECORDED);
    assert!(!m.state().revoked);
    // A change to another role set leaves the epoch, and the votes, alone.
    m.add_role(&ROLE_GUARDIAN, &fx.account(17));
    assert_eq!(m.config().recovery_epoch, 1);
    assert_eq!(m.recovery_revoke(&fx.recovery), ACTION_RAN);
    assert!(m.state().revoked);
}

#[test]
fn rotation_targets_and_callers_checked_in_order() {
    let fx = Fx::new();
    let (m, r2, a2) = two_of_two(&fx);
    let a3 = fx.account(13);
    let stranger = fx.account(40);
    for who in [
        &stranger,
        &fx.principal,
        &fx.agent,
        &fx.approver,
        &fx.guardian,
        &fx.executor,
        &fx.fee_address,
    ] {
        assert_eq!(
            code(m.try_recovery_rotate_agent(who, &fx.agent, &Some(a3.clone()))),
            MandateError::Unauthorized
        );
        assert_eq!(code(m.try_recovery_revoke(who)), MandateError::Unauthorized);
    }
    // `old` must be an agent and `new` must not be one, before any role
    // conflict of `new`.
    assert_eq!(
        code(m.try_recovery_rotate_agent(&r2, &stranger, &Some(fx.approver.clone()))),
        MandateError::ConfigInvalid
    );
    assert_eq!(
        code(m.try_recovery_rotate_agent(&r2, &fx.agent, &Some(a2.clone()))),
        MandateError::ConfigInvalid
    );
    assert_eq!(
        code(m.try_recovery_rotate_agent(&r2, &fx.agent, &Some(fx.agent.clone()))),
        MandateError::ConfigInvalid
    );
    for who in [
        &fx.approver,
        &fx.executor,
        &fx.recovery,
        &fx.principal,
        &fx.fee_address,
    ] {
        assert_eq!(
            code(m.try_recovery_rotate_agent(&r2, &fx.agent, &Some(who.clone()))),
            MandateError::RoleConflict
        );
    }
    // A guardian may become an agent.
    m.recovery_rotate_agent(&r2, &fx.agent, &Some(fx.guardian.clone()));
    assert_eq!(
        code(m.try_recovery_rotate_agent(&r2, &fx.agent, &Some(fx.guardian.clone()))),
        MandateError::AlreadyApproved
    );
}

#[test]
fn recovery_allowed_while_paused_and_after_expiry() {
    let fx = Fx::new();
    let (m, r2, _) = two_of_two(&fx);
    m.pause(&fx.recovery);
    fx.set_time(NOW + 400 * 86_400);
    m.recovery_rotate_agent(&fx.recovery, &fx.agent, &None);
    assert_eq!(m.recovery_rotate_agent(&r2, &fx.agent, &None), ACTION_RAN);
    m.recovery_revoke(&fx.recovery);
    assert_eq!(m.recovery_revoke(&r2), ACTION_RAN);
}
