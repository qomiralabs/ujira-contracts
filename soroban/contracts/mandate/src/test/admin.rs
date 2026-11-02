// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The principal's administration: limits, roles and their gates, the
//! allowlist, the attestation policy, the principal transfer and the fee
//! cap.

extern crate std;

use super::{code, dest, key_of, unpayable, zero_contract, Fx, DAY, FUNDED, NOW};
use crate::events::{
    ApprovalsRequiredSet, AttestationPolicySet, FeeCapLowered, LimitsUpdated,
    PrincipalProposalCancelled, PrincipalProposed, PrincipalTransferred, RecipientAllowed,
    RecipientDisallowed, RecoveryThresholdSet, RoleAdded, RoleRemoved,
};
use crate::storage::{self, Agent};
use crate::types::{
    AllowEntry, ROLE_AGENT, ROLE_APPROVER, ROLE_EXECUTOR, ROLE_GUARDIAN, ROLE_RECOVERY,
    STATUS_APPROVED, STATUS_PENDING,
};
use crate::{MandateError, UjiraMandateClient};
use mandate_config::{MODE_ATTESTED, MODE_BOTH};
use soroban_sdk::{Address, Bytes, BytesN, Event as _, Vec};

fn tenant(fx: &Fx) -> BytesN<32> {
    fx.b32(0x7e)
}

fn stored_agents(fx: &Fx, m: &Address) -> std::vec::Vec<Agent> {
    fx.env
        .as_contract(m, || storage::roles(&fx.env).unwrap())
        .agents
        .iter()
        .collect()
}

/// Removing an agent releases everything its open payouts reserve at once,
/// whatever their number and status, and touches no other agent's. A later
/// reject or cancel of one of those payouts ends it without releasing it a
/// second time.
#[test]
fn remove_agent_releases_its_reservations() {
    let fx = Fx::new();
    let other = fx.account(21);
    let mut config = fx.config();
    config.agents = fx.list(&[&fx.agent, &other]);
    let m = fx.ready_with(&config);
    let (pending, approved, theirs, kept) = (fx.b32(1), fx.b32(2), fx.b32(3), fx.b32(4));
    assert_eq!(m.request_payout(&fx.agent, &fx.req(1, 500)), STATUS_PENDING);
    assert_eq!(
        m.request_payout(&fx.agent, &fx.req(2, 100)),
        STATUS_APPROVED
    );
    assert_eq!(m.request_payout(&other, &fx.req(3, 200)), STATUS_APPROVED);
    assert_eq!(m.request_payout(&other, &fx.req(4, 500)), STATUS_PENDING);
    let st = m.state();
    assert_eq!(
        (st.pending, st.committed, st.free),
        (1_000, 300, FUNDED - 1_300)
    );

    m.remove_role(&ROLE_AGENT, &fx.agent);
    let st = m.state();
    assert_eq!(
        (st.pending, st.committed, st.free),
        (500, 200, FUNDED - 700)
    );
    assert_eq!(st.spent, 0);

    // The records stay open and cannot move on.
    assert_eq!(fx.record(&m, 1).status, STATUS_PENDING);
    assert_eq!(fx.record(&m, 2).status, STATUS_APPROVED);
    assert_eq!(
        code(m.try_approve(&fx.approver, &pending)),
        MandateError::AgentRemoved
    );
    assert_eq!(
        code(m.try_execute(&fx.executor, &approved)),
        MandateError::AgentRemoved
    );

    // Adding the same address back does not revive them or their
    // reservations, and what it queues now is its own.
    m.add_role(&ROLE_AGENT, &fx.agent);
    assert_eq!(m.state().committed, 200);
    assert_eq!(
        code(m.try_execute(&fx.executor, &approved)),
        MandateError::AgentRemoved
    );
    assert_eq!(m.request_payout(&fx.agent, &fx.req(5, 50)), STATUS_APPROVED);
    assert_eq!(m.state().committed, 250);

    // Ending them releases nothing again.
    m.reject(&fx.approver, &pending);
    m.cancel(&fx.principal, &approved);
    let st = m.state();
    assert_eq!((st.pending, st.committed), (500, 250));

    // The other agent's payouts are untouched and move on as usual.
    assert_eq!(m.approve(&fx.approver, &kept), STATUS_APPROVED);
    let st = m.state();
    assert_eq!((st.pending, st.committed), (0, 750));
    for id in [&theirs, &kept, &fx.b32(5)] {
        m.cancel(&fx.principal, id);
    }
    let st = m.state();
    assert_eq!((st.spent, st.pending, st.committed), (0, 0, 0));
    assert_eq!(st.free, FUNDED);
}

/// Released reservations stop counting against the budget too.
#[test]
fn remove_agent_gives_its_budget_back() {
    let fx = Fx::new();
    let other = fx.account(21);
    let mut config = fx.config();
    config.agents = fx.list(&[&fx.agent, &other]);
    config.limits.total_budget = 300;
    config.limits.per_payment_cap = 300;
    let m = fx.ready_with(&config);
    m.request_payout(&fx.agent, &fx.req(1, 300));
    assert_eq!(
        code(m.try_request_payout(&other, &fx.req(2, 100))),
        MandateError::BudgetExceeded
    );
    m.remove_role(&ROLE_AGENT, &fx.agent);
    assert_eq!(m.request_payout(&other, &fx.req(2, 300)), STATUS_APPROVED);
    assert_eq!(m.state().committed, 300);
}

/// No role on a live mandate can be given to an address nobody can act
/// from, and neither can the principal's seat.
#[test]
fn zero_addresses_cannot_join_a_role_or_be_proposed() {
    let fx = Fx::new();
    let m = fx.mandate();
    let before = m.config();
    for a in [fx.account(0), zero_contract(&fx.env)] {
        for role in [
            ROLE_AGENT,
            ROLE_APPROVER,
            ROLE_GUARDIAN,
            ROLE_EXECUTOR,
            ROLE_RECOVERY,
        ] {
            assert_eq!(
                code(m.try_add_role(&role, &a)),
                MandateError::ConfigInvalid,
                "role {role}, {a:?}"
            );
        }
        assert_eq!(
            code(m.try_propose_principal(&a)),
            MandateError::ConfigInvalid,
            "{a:?}"
        );
    }
    assert_eq!(m.config(), before);
    assert!(fx.events(&m.address).is_empty());
}

#[test]
fn add_role_and_rotation_assign_next_epoch() {
    let fx = Fx::new();
    let m = fx.ready();
    let (a2, a3) = (fx.account(21), fx.account(22));
    assert_eq!(m.config().agent_epoch, 0);
    assert_eq!(stored_agents(&fx, &m.address)[0].epoch, 0);

    m.add_role(&ROLE_AGENT, &a2);
    let c = m.config();
    assert_eq!((c.agent_epoch, c.recovery_epoch), (1, 1));
    // Removing an agent and adding it back gives it a new, higher value.
    m.remove_role(&ROLE_AGENT, &fx.agent);
    assert_eq!(m.config().recovery_epoch, 2);
    m.add_role(&ROLE_AGENT, &fx.agent);
    let c = m.config();
    assert_eq!((c.agent_epoch, c.recovery_epoch), (2, 3));
    assert_eq!(c.agents, fx.list(&[&a2, &fx.agent]));
    assert_eq!(
        stored_agents(&fx, &m.address),
        [
            Agent {
                account: a2.clone(),
                epoch: 1,
                pending: 0,
                committed: 0,
            },
            Agent {
                account: fx.agent.clone(),
                epoch: 2,
                pending: 0,
                committed: 0,
            },
        ]
    );

    // A rotation's new agent takes the next value too, in one epoch step.
    assert_eq!(
        m.recovery_rotate_agent(&fx.recovery, &a2, &Some(a3.clone())),
        2
    );
    let c = m.config();
    assert_eq!((c.agent_epoch, c.recovery_epoch), (3, 4));
    assert_eq!(
        stored_agents(&fx, &m.address),
        [
            Agent {
                account: fx.agent.clone(),
                epoch: 2,
                pending: 0,
                committed: 0,
            },
            Agent {
                account: a3.clone(),
                epoch: 3,
                pending: 0,
                committed: 0,
            },
        ]
    );
    // A rotation without a new agent leaves the counter alone.
    m.recovery_rotate_agent(&fx.recovery, &a3, &None);
    let c = m.config();
    assert_eq!((c.agent_epoch, c.recovery_epoch), (3, 5));
    assert_eq!(c.agents, fx.list(&[&fx.agent]));
}

#[test]
fn readded_agent_cannot_revive_its_old_payouts() {
    let fx = Fx::new();
    let m = fx.ready();
    m.request_payout(&fx.agent, &fx.req(1, 500));
    m.remove_role(&ROLE_AGENT, &fx.agent);
    m.add_role(&ROLE_AGENT, &fx.agent);
    assert_eq!(
        code(m.try_approve(&fx.approver, &fx.b32(1))),
        MandateError::AgentRemoved
    );
}

#[test]
fn add_role_checks_in_order() {
    let fx = Fx::new();
    let m = fx.ready();
    let newbie = fx.account(30);
    assert_eq!(
        code(m.try_add_role(&0, &newbie)),
        MandateError::ConfigInvalid
    );
    assert_eq!(
        code(m.try_add_role(&6, &newbie)),
        MandateError::ConfigInvalid
    );
    assert_eq!(
        code(m.try_add_role(&ROLE_GUARDIAN, &fx.guardian)),
        MandateError::ConfigInvalid
    );
    for (role, who) in [
        (1, &fx.principal),
        (4, &fx.principal),
        (5, &fx.principal),
        (2, &fx.agent),
        (1, &fx.approver),
        (4, &fx.approver),
        (2, &fx.executor),
        (5, &fx.agent),
        (5, &fx.executor),
        (1, &fx.recovery),
        (4, &fx.recovery),
        (1, &fx.executor),
        (4, &fx.agent),
        (3, &fx.fee_address),
    ] {
        assert_eq!(
            code(m.try_add_role(&role, who)),
            MandateError::RoleConflict,
            "role {role}"
        );
    }
    // Allowed overlaps.
    m.add_role(&ROLE_APPROVER, &fx.principal);
    m.add_role(&ROLE_GUARDIAN, &fx.principal);
    m.add_role(&ROLE_APPROVER, &fx.recovery);
    m.add_role(&ROLE_GUARDIAN, &fx.agent);
    // Bounds: four guardians at most.
    m.add_role(&ROLE_GUARDIAN, &newbie);
    assert_eq!(
        code(m.try_add_role(&ROLE_GUARDIAN, &fx.account(31))),
        MandateError::RoleSetFull
    );
    // Separation comes before the bound.
    assert_eq!(
        code(m.try_add_role(&ROLE_GUARDIAN, &fx.fee_address)),
        MandateError::RoleConflict
    );
}

#[test]
fn first_and_last_approver_move_the_gate() {
    let fx = Fx::new();
    let mut config = fx.config();
    config.approvers = Vec::new(&fx.env);
    config.approvals_required = 0;
    config.limits.approval_threshold = config.limits.per_payment_cap + 1;
    let m = fx.ready_with(&config);
    m.add_role(&ROLE_APPROVER, &fx.approver);
    assert_eq!(
        fx.events(&m.address),
        [
            RoleAdded {
                tenant_tag: tenant(&fx),
                role: ROLE_APPROVER,
                account: fx.approver.clone(),
            }
            .to_xdr(&fx.env, &m.address),
            ApprovalsRequiredSet {
                tenant_tag: tenant(&fx),
                required: 1,
            }
            .to_xdr(&fx.env, &m.address),
        ]
    );
    let second = fx.account(31);
    m.add_role(&ROLE_APPROVER, &second);
    assert_eq!(fx.events(&m.address).len(), 1);
    m.set_approvals_required(&2);
    assert_eq!(
        code(m.try_remove_role(&ROLE_APPROVER, &second)),
        MandateError::ConfigInvalid
    );
    assert_eq!(
        code(m.try_set_approvals_required(&3)),
        MandateError::ConfigInvalid
    );
    assert_eq!(
        code(m.try_set_approvals_required(&0)),
        MandateError::ConfigInvalid
    );
    m.set_approvals_required(&1);
    m.remove_role(&ROLE_APPROVER, &second);
    // The last approver may go only while no payout can need approval.
    let mut limits = m.config().limits;
    limits.approval_threshold = 500;
    m.update_limits(&limits);
    assert_eq!(
        code(m.try_remove_role(&ROLE_APPROVER, &fx.approver)),
        MandateError::ConfigInvalid
    );
    limits.approval_threshold = limits.per_payment_cap + 1;
    m.update_limits(&limits);
    m.remove_role(&ROLE_APPROVER, &fx.approver);
    assert_eq!(
        fx.events(&m.address),
        [
            RoleRemoved {
                tenant_tag: tenant(&fx),
                role: ROLE_APPROVER,
                account: fx.approver.clone(),
            }
            .to_xdr(&fx.env, &m.address),
            ApprovalsRequiredSet {
                tenant_tag: tenant(&fx),
                required: 0,
            }
            .to_xdr(&fx.env, &m.address),
        ]
    );
    assert_eq!(m.config().approvals_required, 0);
}

#[test]
fn recovery_threshold_follows_the_set() {
    let fx = Fx::new();
    let mut config = fx.config();
    config.recovery = Vec::new(&fx.env);
    config.recovery_threshold = 0;
    let m = fx.ready_with(&config);
    let r2 = fx.account(32);
    assert_eq!(
        code(m.try_set_recovery_threshold(&1)),
        MandateError::ConfigInvalid
    );

    m.add_role(&ROLE_RECOVERY, &fx.recovery);
    assert_eq!(
        fx.events(&m.address),
        [
            RoleAdded {
                tenant_tag: tenant(&fx),
                role: ROLE_RECOVERY,
                account: fx.recovery.clone(),
            }
            .to_xdr(&fx.env, &m.address),
            RecoveryThresholdSet {
                tenant_tag: tenant(&fx),
                required: 1,
                recovery_epoch: 1,
            }
            .to_xdr(&fx.env, &m.address),
        ]
    );
    m.add_role(&ROLE_RECOVERY, &r2);
    assert_eq!(fx.events(&m.address).len(), 1);
    let c = m.config();
    assert_eq!((c.recovery_threshold, c.recovery_epoch), (1, 2));
    assert_eq!(
        code(m.try_set_recovery_threshold(&0)),
        MandateError::ConfigInvalid
    );
    assert_eq!(
        code(m.try_set_recovery_threshold(&3)),
        MandateError::ConfigInvalid
    );
    m.set_recovery_threshold(&2);
    assert_eq!(
        fx.events(&m.address),
        [RecoveryThresholdSet {
            tenant_tag: tenant(&fx),
            required: 2,
            recovery_epoch: 3,
        }
        .to_xdr(&fx.env, &m.address)]
    );
    // Removing one of two below the threshold is refused.
    assert_eq!(
        code(m.try_remove_role(&ROLE_RECOVERY, &r2)),
        MandateError::ConfigInvalid
    );
    m.set_recovery_threshold(&1);
    m.remove_role(&ROLE_RECOVERY, &r2);
    // The last member may always go, and the threshold follows it to 0.
    m.remove_role(&ROLE_RECOVERY, &fx.recovery);
    assert_eq!(
        fx.events(&m.address),
        [
            RoleRemoved {
                tenant_tag: tenant(&fx),
                role: ROLE_RECOVERY,
                account: fx.recovery.clone(),
            }
            .to_xdr(&fx.env, &m.address),
            RecoveryThresholdSet {
                tenant_tag: tenant(&fx),
                required: 0,
                recovery_epoch: 6,
            }
            .to_xdr(&fx.env, &m.address),
        ]
    );
    let c = m.config();
    assert_eq!((c.recovery_threshold, c.recovery.len()), (0, 0));
    assert_eq!(
        code(m.try_recovery_revoke(&fx.recovery)),
        MandateError::Unauthorized
    );
}

#[test]
fn role_lists_keep_insertion_order() {
    let fx = Fx::new();
    let m = fx.ready();
    let (g2, g3, g4) = (fx.account(33), fx.account(34), fx.account(35));
    for g in [&g2, &g3, &g4] {
        m.add_role(&ROLE_GUARDIAN, g);
    }
    m.remove_role(&ROLE_GUARDIAN, &g2);
    m.add_role(&ROLE_GUARDIAN, &g2);
    assert_eq!(
        m.config().guardians,
        fx.list(&[&fx.guardian, &g3, &g4, &g2])
    );
    assert_eq!(
        code(m.try_remove_role(&ROLE_GUARDIAN, &fx.account(36))),
        MandateError::ConfigInvalid
    );
    assert_eq!(
        code(m.try_remove_role(&ROLE_APPROVER, &fx.agent)),
        MandateError::ConfigInvalid
    );
}

#[test]
fn update_limits_checks_in_order() {
    let fx = Fx::new();
    let m = fx.ready();
    m.request_payout(&fx.agent, &fx.req(1, 500));
    let mut limits = m.config().limits;
    limits.per_payment_cap = limits.total_budget + 1;
    assert_eq!(
        code(m.try_update_limits(&limits)),
        MandateError::ConfigInvalid
    );
    let mut limits = m.config().limits;
    limits.expires_at = NOW;
    assert_eq!(
        code(m.try_update_limits(&limits)),
        MandateError::ConfigInvalid
    );
    let mut limits = m.config().limits;
    limits.total_budget = 499;
    limits.per_payment_cap = 499;
    assert_eq!(
        code(m.try_update_limits(&limits)),
        MandateError::BudgetBelowReserved
    );
    limits.total_budget = 500;
    m.update_limits(&limits);
    assert_eq!(
        fx.events(&m.address),
        [LimitsUpdated {
            tenant_tag: tenant(&fx),
            total_budget: 500,
            per_payment_cap: 499,
            per_recipient_daily_cap: limits.per_recipient_daily_cap,
            approval_threshold: limits.approval_threshold,
            payout_ttl_seconds: limits.payout_ttl_seconds,
            expires_at: limits.expires_at,
        }
        .to_xdr(&fx.env, &m.address)]
    );
    assert_eq!(m.config().limits, limits);
}

#[test]
fn reactivation_needs_the_reservations_covered() {
    let fx = Fx::new();
    let mut config = fx.config();
    config.principal = <Address as soroban_sdk::testutils::Address>::generate(&fx.env);
    config.limits.expires_at = NOW + DAY;
    config.limits.payout_ttl_seconds = 30 * DAY;
    let m = fx.ready_with(&config);
    m.request_payout(&fx.agent, &fx.req(1, 500));
    fx.set_time(NOW + DAY);
    // Expired, so the whole balance is free and the principal takes most.
    m.withdraw(&(FUNDED - 499));
    let mut limits = m.config().limits;
    limits.expires_at = NOW + 10 * DAY;
    assert_eq!(
        code(m.try_update_limits(&limits)),
        MandateError::InsufficientFunds
    );
    fx.mint(&m.address, 1);
    m.update_limits(&limits);
    assert!(!m.state().expired);
}

#[test]
fn no_approvers_needs_the_threshold_above_the_cap() {
    let fx = Fx::new();
    let mut config = fx.config();
    config.approvers = Vec::new(&fx.env);
    config.approvals_required = 0;
    config.limits.approval_threshold = 1_001;
    let m = fx.ready_with(&config);
    let mut limits = m.config().limits;
    limits.approval_threshold = 1_000;
    assert_eq!(
        code(m.try_update_limits(&limits)),
        MandateError::ConfigInvalid
    );
    limits.approval_threshold = 2_000;
    limits.per_payment_cap = 2_000;
    assert_eq!(
        code(m.try_update_limits(&limits)),
        MandateError::ConfigInvalid
    );
    limits.per_payment_cap = 1_999;
    m.update_limits(&limits);
}

fn entry(fx: &Fx, recipient: u8, seed: u8, tag: Option<u64>) -> AllowEntry {
    AllowEntry {
        recipient_ref: fx.b32(recipient),
        destination: dest(&fx.env, seed, tag),
    }
}

fn entries(fx: &Fx, items: &[AllowEntry]) -> Vec<AllowEntry> {
    let mut out = Vec::new(&fx.env);
    for e in items {
        out.push_back(e.clone());
    }
    out
}

/// Binding a recipient to an address nobody can spend from is refused on
/// allow and on disallow, beside a good entry too, so no such entry can
/// ever be on the list for a payout to use.
#[test]
fn allowlist_entries_to_unpayable_addresses_refused() {
    let fx = Fx::new();
    let m = fx.mandate();
    let good = entry(&fx, 1, 60, None);
    // Present, so only the bad entry beside it can refuse a disallow.
    let listed = entry(&fx, 1, 61, None);
    m.allow_recipients(&entries(&fx, core::slice::from_ref(&listed)));
    for a in unpayable(&fx, &m.address) {
        let bad = AllowEntry {
            recipient_ref: fx.b32(1),
            destination: soroban_sdk::MuxedAddress::from(a.clone()),
        };
        assert_eq!(
            code(m.try_allow_recipients(&entries(&fx, &[good.clone(), bad.clone()]))),
            MandateError::ConfigInvalid,
            "{a:?}"
        );
        assert_eq!(
            code(m.try_disallow_recipients(&entries(&fx, &[listed.clone(), bad]))),
            MandateError::ConfigInvalid,
            "{a:?}"
        );
    }
    let zero_tagged = AllowEntry {
        recipient_ref: fx.b32(1),
        destination: <soroban_sdk::MuxedAddress as soroban_sdk::testutils::MuxedAddress>::new(
            fx.account(0),
            5,
        ),
    };
    assert_eq!(
        code(m.try_allow_recipients(&entries(&fx, &[zero_tagged]))),
        MandateError::ConfigInvalid
    );
    assert!(fx.events(&m.address).is_empty());
    assert!(m.is_allowed(&listed.recipient_ref, &listed.destination));
    assert!(!m.is_allowed(&good.recipient_ref, &good.destination));
    m.allow_recipients(&entries(&fx, core::slice::from_ref(&good)));
    assert!(m.is_allowed(&good.recipient_ref, &good.destination));
}

#[test]
fn allowlist_entries_checked_and_applied_atomically() {
    let fx = Fx::new();
    let m = fx.mandate();
    let a = entry(&fx, 1, 60, None);
    let b = entry(&fx, 1, 60, Some(7));
    let c = entry(&fx, 2, 60, Some(0));
    assert_eq!(
        code(m.try_allow_recipients(&Vec::new(&fx.env))),
        MandateError::BatchInvalid
    );
    let eleven: std::vec::Vec<AllowEntry> =
        (0..11u8).map(|i| entry(&fx, 3, 70 + i, None)).collect();
    assert_eq!(
        code(m.try_allow_recipients(&entries(&fx, &eleven))),
        MandateError::BatchInvalid
    );
    m.allow_recipients(&entries(&fx, &eleven[..10]));
    let fee = AllowEntry {
        recipient_ref: fx.b32(1),
        destination: <soroban_sdk::MuxedAddress as soroban_sdk::testutils::MuxedAddress>::new(
            &fx.fee_address,
            5,
        ),
    };
    assert_eq!(
        code(m.try_allow_recipients(&entries(&fx, &[a.clone(), fee]))),
        MandateError::ConfigInvalid
    );
    // A pair given twice counts as present for its second copy.
    assert_eq!(
        code(m.try_allow_recipients(&entries(&fx, &[a.clone(), b.clone(), a.clone()]))),
        MandateError::ConfigInvalid
    );
    assert!(!m.is_allowed(&a.recipient_ref, &a.destination));

    m.allow_recipients(&entries(&fx, &[a.clone(), b.clone(), c.clone()]));
    let events: std::vec::Vec<_> = [&a, &b, &c]
        .iter()
        .map(|e| {
            RecipientAllowed {
                tenant_tag: tenant(&fx),
                recipient_ref: e.recipient_ref.clone(),
                destination: e.destination.clone(),
            }
            .to_xdr(&fx.env, &m.address)
        })
        .collect();
    assert_eq!(fx.events(&m.address), events);
    // The tag is part of the pair: no tag, tag 7 and tag 0 are three entries.
    assert!(m.is_allowed(&a.recipient_ref, &a.destination));
    assert!(m.is_allowed(&b.recipient_ref, &b.destination));
    assert!(!m.is_allowed(&fx.b32(2), dest(&fx.env, 60, None)));
    assert_eq!(
        code(m.try_allow_recipients(&entries(&fx, core::slice::from_ref(&a)))),
        MandateError::ConfigInvalid
    );

    let gone = entry(&fx, 9, 61, None);
    assert_eq!(
        code(m.try_disallow_recipients(&entries(&fx, &[b.clone(), gone]))),
        MandateError::ConfigInvalid
    );
    assert_eq!(
        code(m.try_disallow_recipients(&entries(&fx, &[b.clone(), b.clone()]))),
        MandateError::ConfigInvalid
    );
    m.disallow_recipients(&entries(&fx, core::slice::from_ref(&b)));
    assert_eq!(
        fx.events(&m.address),
        [RecipientDisallowed {
            tenant_tag: tenant(&fx),
            recipient_ref: b.recipient_ref.clone(),
            destination: b.destination.clone(),
        }
        .to_xdr(&fx.env, &m.address)]
    );
    assert!(!m.is_allowed(&b.recipient_ref, &b.destination));
    assert!(m.is_allowed(&a.recipient_ref, &a.destination));
}

fn attested_config(fx: &Fx, mode: u32) -> crate::MandateConfig {
    let mut config = fx.config();
    config.recipient_mode = mode;
    config.attester = Some(key_of(&fx.env, 0xa7));
    config
}

#[test]
fn allowlist_refused_in_attested_mode() {
    let fx = Fx::new();
    let id = fx.create(&attested_config(&fx, MODE_ATTESTED)).unwrap();
    let m = UjiraMandateClient::new(&fx.env, &id);
    let a = entry(&fx, 1, 60, None);
    assert_eq!(
        code(m.try_allow_recipients(&Vec::new(&fx.env))),
        MandateError::WrongRecipientMode
    );
    assert_eq!(
        code(m.try_disallow_recipients(&entries(&fx, &[a]))),
        MandateError::WrongRecipientMode
    );
}

#[test]
fn attestation_policy_changes_bump_the_epoch() {
    let fx = Fx::new();
    let m = fx.mandate();
    let key = key_of(&fx.env, 0xa8);
    assert_eq!(
        code(m.try_set_attestation_policy(&key, &fx.b32(1))),
        MandateError::WrongRecipientMode
    );

    let id = fx.create(&attested_config(&fx, MODE_BOTH)).unwrap();
    let m = UjiraMandateClient::new(&fx.env, &id);
    for bad in [
        Bytes::from_array(&fx.env, &[0u8; 32]),
        Bytes::from_array(&fx.env, &[1u8; 31]),
        Bytes::from_array(&fx.env, &[1u8; 33]),
    ] {
        assert_eq!(
            code(m.try_set_attestation_policy(&bad, &fx.b32(1))),
            MandateError::ConfigInvalid
        );
    }
    // The key of the principal, the fee address or any role account.
    for seed in [1u8, 2, 3, 4, 5, 6, 7] {
        assert_eq!(
            code(m.try_set_attestation_policy(&key_of(&fx.env, seed), &fx.b32(1))),
            MandateError::RoleConflict,
            "seed {seed}"
        );
    }
    m.set_attestation_policy(&key, &fx.b32(1));
    assert_eq!(
        fx.events(&id),
        [AttestationPolicySet {
            tenant_tag: tenant(&fx),
            attester: key.clone(),
            country_policy_hash: fx.b32(1),
            policy_epoch: 1,
        }
        .to_xdr(&fx.env, &id)]
    );
    let c = m.config();
    assert_eq!((c.attester, c.country_policy_hash), (Some(key), fx.b32(1)));
    assert_eq!(m.state().policy_epoch, 1);
    // A role account whose key is the attester's cannot join.
    let holder = fx.account(0xa8);
    assert_eq!(
        code(m.try_add_role(&ROLE_GUARDIAN, &holder)),
        MandateError::RoleConflict
    );
    assert_eq!(
        code(m.try_propose_principal(&holder)),
        MandateError::RoleConflict
    );
}

#[test]
fn principal_moves_in_two_steps() {
    let fx = Fx::new();
    let m = fx.ready();
    let heir = fx.account(44);
    assert_eq!(code(m.try_accept_principal()), MandateError::Unauthorized);
    assert_eq!(
        code(m.try_cancel_principal_proposal()),
        MandateError::ConfigInvalid
    );
    for who in [&fx.agent, &fx.executor, &fx.recovery, &fx.fee_address] {
        assert_eq!(
            code(m.try_propose_principal(who)),
            MandateError::RoleConflict
        );
    }
    m.propose_principal(&heir);
    assert_eq!(
        fx.events(&m.address),
        [PrincipalProposed {
            tenant_tag: tenant(&fx),
            current: fx.principal.clone(),
            proposed: heir.clone(),
        }
        .to_xdr(&fx.env, &m.address)]
    );
    m.cancel_principal_proposal();
    assert_eq!(
        fx.events(&m.address),
        [PrincipalProposalCancelled {
            tenant_tag: tenant(&fx),
            current: fx.principal.clone(),
            proposed: heir.clone(),
        }
        .to_xdr(&fx.env, &m.address)]
    );
    assert_eq!(m.config().proposed_principal, None);

    // A role gained after the proposal is caught at acceptance.
    m.propose_principal(&heir);
    m.add_role(&ROLE_RECOVERY, &heir);
    assert_eq!(code(m.try_accept_principal()), MandateError::RoleConflict);
    m.remove_role(&ROLE_RECOVERY, &heir);
    // Allowed even after revoke.
    m.revoke();
    m.accept_principal();
    assert_eq!(
        fx.events(&m.address),
        [PrincipalTransferred {
            tenant_tag: tenant(&fx),
            previous: fx.principal.clone(),
            current: heir.clone(),
        }
        .to_xdr(&fx.env, &m.address)]
    );
    let c = m.config();
    assert_eq!((c.principal, c.proposed_principal), (heir, None));
}

#[test]
fn fee_cap_only_goes_down() {
    let fx = Fx::new();
    let mut config = fx.config();
    config.max_fee_bps = 50;
    let m = fx.ready_with(&config);
    assert_eq!(code(m.try_lower_fee_cap(&50)), MandateError::ConfigInvalid);
    assert_eq!(code(m.try_lower_fee_cap(&51)), MandateError::ConfigInvalid);
    m.pause(&fx.guardian);
    fx.set_time(NOW + 400 * DAY);
    m.lower_fee_cap(&10);
    assert_eq!(
        fx.events(&m.address),
        [FeeCapLowered {
            tenant_tag: tenant(&fx),
            max_fee_bps: 10,
        }
        .to_xdr(&fx.env, &m.address)]
    );
    m.lower_fee_cap(&0);
    assert_eq!(m.config().max_fee_bps, 0);
    assert_eq!(code(m.try_lower_fee_cap(&0)), MandateError::ConfigInvalid);
}
