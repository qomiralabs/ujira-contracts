// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! `approve` and `reject`: M of N distinct current approvers, the repeat
//! rule, the veto, and refusing the payouts of an agent that is gone.

use super::drop_agent;
use super::{code, Fx, DAY, FUNDED, NOW};
use crate::events::{PayoutApprovalAdded, PayoutApproved, PayoutRejected};
use crate::storage::Agent;
use crate::types::{Approval, ROLE_APPROVER, STATUS_APPROVED, STATUS_PENDING, STATUS_REJECTED};
use crate::{MandateError, UjiraMandateClient};
use soroban_sdk::{Address, Event as _, Vec};

/// A mandate with three approvers, two of whom must approve, and one
/// pending payout of 500.
fn two_of_three(fx: &Fx) -> (UjiraMandateClient<'_>, [Address; 3]) {
    let a = [fx.approver.clone(), fx.account(31), fx.account(32)];
    let mut config = fx.config();
    config.approvers = fx.list(&[&a[0], &a[1], &a[2]]);
    config.approvals_required = 2;
    let m = fx.ready_with(&config);
    assert_eq!(m.request_payout(&fx.agent, &fx.req(1, 500)), STATUS_PENDING);
    (m, a)
}

/// What a record's approvals should hold: who approved, under which epoch.
fn approvals(fx: &Fx, items: &[(&Address, u32)]) -> Vec<Approval> {
    let mut out = Vec::new(&fx.env);
    for (who, epoch) in items {
        out.push_back(Approval {
            approver: (*who).clone(),
            epoch: *epoch,
        });
    }
    out
}

fn pending_one(fx: &Fx) -> UjiraMandateClient<'_> {
    let m = fx.ready();
    assert_eq!(m.request_payout(&fx.agent, &fx.req(1, 500)), STATUS_PENDING);
    m
}

fn totals(m: &UjiraMandateClient) -> (i128, i128, i128) {
    let st = m.state();
    (st.spent, st.pending, st.committed)
}

#[test]
fn approve_one_of_one_moves_the_charge_to_committed() {
    let fx = Fx::new();
    let m = pending_one(&fx);
    assert_eq!(m.approve(&fx.approver, &fx.b32(1)), STATUS_APPROVED);
    assert_eq!(
        fx.events(&m.address),
        [
            PayoutApprovalAdded {
                payout_id: fx.b32(1),
                approver: fx.approver.clone(),
                approvals: 1,
                required: 1,
            }
            .to_xdr(&fx.env, &m.address),
            PayoutApproved {
                payout_id: fx.b32(1),
            }
            .to_xdr(&fx.env, &m.address),
        ]
    );
    assert_eq!(totals(&m), (0, 0, 500));
    let r = fx.record(&m, 1);
    assert_eq!(r.status, STATUS_APPROVED);
    assert_eq!(r.approvals, approvals(&fx, &[(&fx.approver, 0)]));
}

#[test]
fn approve_two_of_three_needs_two_distinct_approvers() {
    let fx = Fx::new();
    let (m, a) = two_of_three(&fx);
    assert_eq!(m.approve(&a[0], &fx.b32(1)), STATUS_PENDING);
    assert_eq!(
        fx.events(&m.address),
        [PayoutApprovalAdded {
            payout_id: fx.b32(1),
            approver: a[0].clone(),
            approvals: 1,
            required: 2,
        }
        .to_xdr(&fx.env, &m.address)]
    );
    // The same approver again changes nothing and is refused.
    assert_eq!(
        code(m.try_approve(&a[0], &fx.b32(1))),
        MandateError::AlreadyApproved
    );
    assert_eq!(totals(&m), (0, 500, 0));
    assert_eq!(m.approve(&a[1], &fx.b32(1)), STATUS_APPROVED);
    assert_eq!(fx.events(&m.address).len(), 2);
    assert_eq!(totals(&m), (0, 0, 500));
}

#[test]
fn approve_repeat_completes_after_requirement_lowered() {
    let fx = Fx::new();
    let (m, a) = two_of_three(&fx);
    m.approve(&a[0], &fx.b32(1));
    fx.edit_roles(&m.address, |r| r.approvals_required = 1);
    // Already approved, and the requirement is now met: the payout moves on
    // without a second approval being added.
    assert_eq!(m.approve(&a[0], &fx.b32(1)), STATUS_APPROVED);
    assert_eq!(
        fx.events(&m.address),
        [PayoutApproved {
            payout_id: fx.b32(1),
        }
        .to_xdr(&fx.env, &m.address)]
    );
    assert_eq!(fx.record(&m, 1).approvals, approvals(&fx, &[(&a[0], 0)]));
}

#[test]
fn approve_from_removed_approver_no_longer_counts() {
    let fx = Fx::new();
    let (m, a) = two_of_three(&fx);
    m.approve(&a[0], &fx.b32(1));
    let gone = a[0].clone();
    fx.edit_roles(&m.address, |r| {
        let i = r.approvers.iter().position(|x| x.account == gone).unwrap();
        r.approvers.remove(u32::try_from(i).unwrap());
    });
    // Only the live approval counts: one, not two.
    assert_eq!(m.approve(&a[1], &fx.b32(1)), STATUS_PENDING);
    assert_eq!(
        fx.events(&m.address),
        [PayoutApprovalAdded {
            payout_id: fx.b32(1),
            approver: a[1].clone(),
            approvals: 1,
            required: 2,
        }
        .to_xdr(&fx.env, &m.address)]
    );
    assert_eq!(m.approve(&a[2], &fx.b32(1)), STATUS_APPROVED);
    // A removed approver cannot approve at all.
    assert_eq!(
        code(m.try_approve(&gone, &fx.b32(1))),
        MandateError::Unauthorized
    );
}

/// An approver removed and added back starts over: what it approved before
/// no longer counts, and it may approve again, which does count.
#[test]
fn approve_from_before_a_readd_does_not_count() {
    let fx = Fx::new();
    let (m, a) = two_of_three(&fx);
    let id = fx.b32(1);
    assert_eq!(m.approve(&a[0], &id), STATUS_PENDING);
    m.remove_role(&ROLE_APPROVER, &a[0]);
    m.add_role(&ROLE_APPROVER, &a[0]);
    // One live approval after this, not two.
    assert_eq!(m.approve(&a[1], &id), STATUS_PENDING);
    assert_eq!(m.config().approver_epoch, 1);
    assert_eq!(
        fx.record(&m, 1).approvals,
        approvals(&fx, &[(&a[0], 0), (&a[1], 0)])
    );
    assert_eq!(
        fx.events(&m.address),
        [PayoutApprovalAdded {
            payout_id: id.clone(),
            approver: a[1].clone(),
            approvals: 1,
            required: 2,
        }
        .to_xdr(&fx.env, &m.address)]
    );
    assert_eq!(totals(&m), (0, 500, 0));
    // Not a repeat: the re-added approver's new approval is what counts.
    assert_eq!(m.approve(&a[0], &id), STATUS_APPROVED);
    assert_eq!(
        fx.events(&m.address),
        [
            PayoutApprovalAdded {
                payout_id: id.clone(),
                approver: a[0].clone(),
                approvals: 2,
                required: 2,
            }
            .to_xdr(&fx.env, &m.address),
            PayoutApproved { payout_id: id }.to_xdr(&fx.env, &m.address),
        ]
    );
    assert_eq!(totals(&m), (0, 0, 500));
    // The old entry is replaced, not kept beside the new one.
    assert_eq!(
        fx.record(&m, 1).approvals,
        approvals(&fx, &[(&a[1], 0), (&a[0], 1)])
    );
}

#[test]
fn approve_check_order() {
    let fx = Fx::new();
    let m = pending_one(&fx);
    let id = fx.b32(1);
    // Only approvers, before anything about the mandate or the payout.
    fx.edit_flags(&m.address, |f| f.revoked = true);
    for who in [&fx.principal, &fx.agent, &fx.guardian, &fx.executor] {
        assert_eq!(code(m.try_approve(who, &id)), MandateError::Unauthorized);
    }
    fx.edit_flags(&m.address, |f| f.paused = true);
    assert_eq!(
        code(m.try_approve(&fx.approver, &id)),
        MandateError::MandateRevoked
    );
    fx.edit_flags(&m.address, |f| f.revoked = false);
    assert_eq!(
        code(m.try_approve(&fx.approver, &id)),
        MandateError::MandatePaused
    );
    fx.edit_flags(&m.address, |f| f.paused = false);
    assert_eq!(
        code(m.try_approve(&fx.approver, &fx.b32(9))),
        MandateError::PayoutNotFound
    );
    // Lapsed comes before the agent check.
    fx.edit_roles(&m.address, |r| drop_agent(r, &fx.agent));
    fx.set_time(NOW + DAY);
    assert_eq!(
        code(m.try_approve(&fx.approver, &id)),
        MandateError::PayoutLapsed
    );
    fx.set_time(NOW + DAY - 1);
    assert_eq!(
        code(m.try_approve(&fx.approver, &id)),
        MandateError::AgentRemoved
    );
}

#[test]
fn approve_refuses_a_payout_that_is_not_pending() {
    let fx = Fx::new();
    let m = fx.ready();
    // Below the threshold, so approved at request.
    m.request_payout(&fx.agent, &fx.req(1, 100));
    assert_eq!(
        code(m.try_approve(&fx.approver, &fx.b32(1))),
        MandateError::PayoutWrongStatus
    );
    m.request_payout(&fx.agent, &fx.req(2, 500));
    m.reject(&fx.approver, &fx.b32(2));
    assert_eq!(
        code(m.try_approve(&fx.approver, &fx.b32(2))),
        MandateError::PayoutWrongStatus
    );
}

#[test]
fn approve_expired_mandate_refused() {
    let fx = Fx::new();
    let mut config = fx.config();
    config.limits.expires_at = NOW + 3_600;
    let m = fx.ready_with(&config);
    m.request_payout(&fx.agent, &fx.req(1, 500));
    fx.set_time(NOW + 3_600);
    assert_eq!(
        code(m.try_approve(&fx.approver, &fx.b32(1))),
        MandateError::MandateExpired
    );
}

#[test]
fn approve_after_remove_role_gives_agent_removed() {
    let fx = Fx::new();
    let m = pending_one(&fx);
    // What remove_role(1, agent) leaves behind.
    fx.edit_roles(&m.address, |r| drop_agent(r, &fx.agent));
    assert_eq!(
        code(m.try_approve(&fx.approver, &fx.b32(1))),
        MandateError::AgentRemoved
    );
    // The principal can still cancel it.
    m.cancel(&fx.principal, &fx.b32(1));
    assert_eq!(totals(&m), (0, 0, 0));
}

#[test]
fn approve_after_recovery_rotation_gives_agent_removed() {
    let fx = Fx::new();
    let m = pending_one(&fx);
    let new = fx.account(22);
    // What a recovery rotation leaves behind: the old agent out, the new
    // one in under the next epoch.
    fx.edit_roles(&m.address, |r| {
        drop_agent(r, &fx.agent);
        r.agent_epoch += 1;
        r.agents.push_back(Agent {
            account: new.clone(),
            epoch: r.agent_epoch,
        });
        r.recovery_epoch += 1;
    });
    assert_eq!(
        code(m.try_approve(&fx.approver, &fx.b32(1))),
        MandateError::AgentRemoved
    );
    // The new agent's own payouts go through.
    assert_eq!(m.request_payout(&new, &fx.req(2, 500)), STATUS_PENDING);
    assert_eq!(m.approve(&fx.approver, &fx.b32(2)), STATUS_APPROVED);
    // Anyone may cancel the old one once it lapses.
    fx.set_time(NOW + DAY);
    let stranger = fx.account(40);
    m.cancel(&stranger, &fx.b32(1));
}

#[test]
fn approve_after_readd_gives_agent_removed() {
    let fx = Fx::new();
    let m = pending_one(&fx);
    // Removed, then added back: it returns under a later epoch.
    fx.edit_roles(&m.address, |r| {
        drop_agent(r, &fx.agent);
        r.agent_epoch += 1;
        r.agents.push_back(Agent {
            account: fx.agent.clone(),
            epoch: r.agent_epoch,
        });
    });
    assert_eq!(
        code(m.try_approve(&fx.approver, &fx.b32(1))),
        MandateError::AgentRemoved
    );
    // Nor may the re-added agent cancel its old payout as its agent.
    assert_eq!(
        code(m.try_cancel(&fx.agent, &fx.b32(1))),
        MandateError::Unauthorized
    );
    // Its new work is its own.
    m.request_payout(&fx.agent, &fx.req(2, 500));
    assert_eq!(m.approve(&fx.approver, &fx.b32(2)), STATUS_APPROVED);
    m.cancel(&fx.agent, &fx.b32(2));
    m.cancel(&fx.principal, &fx.b32(1));
    assert_eq!(totals(&m), (0, 0, 0));
}

#[test]
fn reject_by_approver_or_principal_releases_the_charge() {
    let fx = Fx::new();
    let mut config = fx.config();
    config.max_fee_bps = 100;
    let m = fx.ready_with(&config);
    // A total of 496 is below the threshold of 500; with its fee of 4 the
    // charge is not.
    let mut req = fx.req(1, 496);
    req.fee = 4;
    assert_eq!(m.request_payout(&fx.agent, &req), STATUS_PENDING);
    assert_eq!(totals(&m), (0, 500, 0));
    m.reject(&fx.approver, &fx.b32(1));
    assert_eq!(
        fx.events(&m.address),
        [PayoutRejected {
            payout_id: fx.b32(1),
            by: fx.approver.clone(),
        }
        .to_xdr(&fx.env, &m.address)]
    );
    assert_eq!(totals(&m), (0, 0, 0));
    assert_eq!(fx.tombstone(&m, 1).status, STATUS_REJECTED);
    // Nothing moved.
    assert_eq!(m.state().balance, FUNDED);

    m.request_payout(&fx.agent, &fx.req(2, 500));
    m.reject(&fx.principal, &fx.b32(2));
    assert_eq!(fx.tombstone(&m, 2).status, STATUS_REJECTED);
}

#[test]
fn reject_check_order() {
    let fx = Fx::new();
    let m = pending_one(&fx);
    let id = fx.b32(1);
    fx.edit_flags(&m.address, |f| f.revoked = true);
    for who in [&fx.agent, &fx.guardian, &fx.executor, &fx.recovery] {
        assert_eq!(code(m.try_reject(who, &id)), MandateError::Unauthorized);
    }
    assert_eq!(
        code(m.try_reject(&fx.principal, &id)),
        MandateError::MandateRevoked
    );
    fx.edit_flags(&m.address, |f| f.revoked = false);
    assert_eq!(
        code(m.try_reject(&fx.approver, &fx.b32(9))),
        MandateError::PayoutNotFound
    );
    m.request_payout(&fx.agent, &fx.req(2, 100));
    assert_eq!(
        code(m.try_reject(&fx.approver, &fx.b32(2))),
        MandateError::PayoutWrongStatus
    );
}

#[test]
fn reject_allowed_while_paused_after_expiry_and_after_lapse() {
    let fx = Fx::new();
    let mut config = fx.config();
    config.limits.expires_at = NOW + 3_600;
    let m = fx.ready_with(&config);
    m.request_payout(&fx.agent, &fx.req(1, 500));
    m.request_payout(&fx.agent, &fx.req(2, 500));
    fx.edit_flags(&m.address, |f| f.paused = true);
    m.reject(&fx.approver, &fx.b32(1));
    fx.set_time(NOW + DAY);
    m.reject(&fx.principal, &fx.b32(2));
    assert_eq!(
        m.state(),
        crate::types::StateView {
            spent: 0,
            pending: 0,
            committed: 0,
            balance: FUNDED,
            free: FUNDED,
            paused: true,
            revoked: false,
            expired: true,
            policy_epoch: 0,
        }
    );
}

#[test]
fn approve_counts_each_approval_once_in_totals() {
    // Pending and committed always equal the charges of the payouts in
    // each status.
    let fx = Fx::new();
    let (m, a) = two_of_three(&fx);
    m.request_payout(&fx.agent, &fx.req(2, 499));
    m.request_payout(&fx.agent, &fx.req(3, 500));
    m.approve(&a[0], &fx.b32(1));
    m.approve(&a[1], &fx.b32(1));
    m.approve(&a[2], &fx.b32(3));
    assert_eq!(totals(&m), (0, 500, 999));
}
