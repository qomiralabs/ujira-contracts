// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! `cancel` and `cancel_batch`: who may cancel and why, what a cancel
//! releases, and the batch running per item.

use super::{code, Fx, DAY, FUNDED, NOW};
use crate::events::PayoutCancelled;
use crate::payout::{CANCEL_BY_AGENT, CANCEL_BY_PRINCIPAL, CANCEL_LAPSED, SWEEP_GRACE_SECONDS};
use crate::storage::Agent;
use crate::types::{ItemResult, ROLE_AGENT, STATUS_CANCELLED, STATUS_PENDING};
use crate::{MandateError, UjiraMandateClient};
use soroban_sdk::{Address, BytesN, Event as _, Vec};

fn cancelled(fx: &Fx, m: &UjiraMandateClient, id: u8, by: &Address, reason: u32) {
    assert_eq!(
        fx.events(&m.address),
        [PayoutCancelled {
            payout_id: fx.b32(id),
            by: by.clone(),
            reason,
        }
        .to_xdr(&fx.env, &m.address)]
    );
    assert_eq!(fx.tombstone(m, id).status, STATUS_CANCELLED);
}

#[test]
fn cancel_by_principal_and_own_agent_release_the_charge() {
    let fx = Fx::new();
    let mut config = fx.config();
    config.max_fee_bps = 100;
    config.limits.per_recipient_daily_cap = 1_000;
    let m = fx.ready_with(&config);
    let mut approved = fx.req(1, 300);
    approved.fee = 3;
    let mut pending = fx.req(2, 600);
    pending.fee = 6;
    m.request_payout(&fx.agent, &approved);
    assert_eq!(m.request_payout(&fx.agent, &pending), STATUS_PENDING);
    let st = m.state();
    assert_eq!((st.pending, st.committed), (606, 303));

    m.cancel(&fx.principal, &fx.b32(1));
    cancelled(&fx, &m, 1, &fx.principal, CANCEL_BY_PRINCIPAL);
    assert_eq!(m.state().committed, 0);
    m.cancel(&fx.agent, &fx.b32(2));
    cancelled(&fx, &m, 2, &fx.agent, CANCEL_BY_AGENT);
    let st = m.state();
    assert_eq!((st.spent, st.pending, st.committed), (0, 0, 0));
    // Nothing moved; no fee was paid.
    assert_eq!((st.balance, st.free), (FUNDED, FUNDED));
}

#[test]
fn cancel_by_another_agent_or_any_role_refused_until_lapse() {
    let fx = Fx::new();
    let m = fx.ready();
    let other = fx.account(21);
    fx.edit_roles(&m.address, |r| {
        r.agent_epoch += 1;
        r.agents.push_back(Agent {
            account: other.clone(),
            epoch: r.agent_epoch,
            pending: 0,
            committed: 0,
        });
    });
    m.request_payout(&fx.agent, &fx.req(1, 500));
    let stranger = fx.account(40);
    for who in [
        &other,
        &fx.approver,
        &fx.guardian,
        &fx.executor,
        &fx.recovery,
        &fx.fee_address,
        &stranger,
    ] {
        assert_eq!(
            code(m.try_cancel(who, &fx.b32(1))),
            MandateError::Unauthorized
        );
    }
    // At the payout's own expiry anyone may.
    fx.set_time(NOW + DAY - 1);
    assert_eq!(
        code(m.try_cancel(&stranger, &fx.b32(1))),
        MandateError::Unauthorized
    );
    fx.set_time(NOW + DAY);
    m.cancel(&stranger, &fx.b32(1));
    cancelled(&fx, &m, 1, &stranger, CANCEL_LAPSED);
    assert_eq!(m.state().pending, 0);
}

#[test]
fn cancel_own_agent_is_reason_2_even_after_lapse() {
    let fx = Fx::new();
    let m = fx.ready();
    m.request_payout(&fx.agent, &fx.req(1, 100));
    fx.set_time(NOW + 2 * DAY);
    m.cancel(&fx.agent, &fx.b32(1));
    cancelled(&fx, &m, 1, &fx.agent, CANCEL_BY_AGENT);
}

#[test]
fn cancel_reason_3_waits_out_grace_when_only_mandate_expired() {
    let fx = Fx::new();
    let mut config = fx.config();
    config.limits.expires_at = NOW + 3_600;
    config.limits.payout_ttl_seconds = 30 * DAY;
    let m = fx.ready_with(&config);
    m.request_payout(&fx.agent, &fx.req(1, 100));
    m.request_payout(&fx.agent, &fx.req(2, 100));
    let stranger = fx.account(40);
    // The mandate has expired but the payout has not lapsed and the grace
    // has not run: a stranger may not cancel yet.
    for now in [NOW + 3_600, NOW + 30 * DAY - 1] {
        fx.set_time(now);
        assert!(now - (NOW + 3_600) < SWEEP_GRACE_SECONDS);
        assert_eq!(
            code(m.try_cancel(&stranger, &fx.b32(1))),
            MandateError::Unauthorized,
            "at {now}"
        );
    }
    // The principal and the agent never wait.
    m.cancel(&fx.agent, &fx.b32(2));
    cancelled(&fx, &m, 2, &fx.agent, CANCEL_BY_AGENT);
    // A payout lives at most as long as the grace, so its own lapse always
    // comes first and frees it for anyone.
    fx.set_time(NOW + 30 * DAY);
    m.cancel(&stranger, &fx.b32(1));
    cancelled(&fx, &m, 1, &stranger, CANCEL_LAPSED);
}

#[test]
fn cancel_check_order() {
    let fx = Fx::new();
    let m = fx.ready();
    m.request_payout(&fx.agent, &fx.req(1, 100));
    let stranger = fx.account(40);
    // Revoked first, for anyone, even for an unknown ID.
    fx.edit_flags(&m.address, |f| f.revoked = true);
    for who in [&fx.principal, &fx.agent, &stranger] {
        assert_eq!(
            code(m.try_cancel(who, &fx.b32(9))),
            MandateError::MandateRevoked
        );
    }
    fx.edit_flags(&m.address, |f| f.revoked = false);
    // Then the lookup and the status, before who is asking.
    assert_eq!(
        code(m.try_cancel(&stranger, &fx.b32(9))),
        MandateError::PayoutNotFound
    );
    m.cancel(&fx.principal, &fx.b32(1));
    assert_eq!(
        code(m.try_cancel(&stranger, &fx.b32(1))),
        MandateError::PayoutWrongStatus
    );
    assert_eq!(
        code(m.try_cancel(&fx.principal, &fx.b32(1))),
        MandateError::PayoutWrongStatus
    );
}

#[test]
fn cancel_allowed_while_paused() {
    let fx = Fx::new();
    let m = fx.ready();
    m.request_payout(&fx.agent, &fx.req(1, 100));
    fx.edit_flags(&m.address, |f| f.paused = true);
    m.cancel(&fx.agent, &fx.b32(1));
    cancelled(&fx, &m, 1, &fx.agent, CANCEL_BY_AGENT);
}

#[test]
fn cancel_removed_agent_refused_principal_allowed() {
    let fx = Fx::new();
    let m = fx.ready();
    m.request_payout(&fx.agent, &fx.req(1, 100));
    m.remove_role(&ROLE_AGENT, &fx.agent);
    assert_eq!(
        code(m.try_cancel(&fx.agent, &fx.b32(1))),
        MandateError::Unauthorized
    );
    m.cancel(&fx.principal, &fx.b32(1));
    cancelled(&fx, &m, 1, &fx.principal, CANCEL_BY_PRINCIPAL);
}

#[test]
fn cancel_batch_runs_per_item() {
    let fx = Fx::new();
    let m = fx.ready();
    m.request_payout(&fx.agent, &fx.req(1, 100));
    m.request_payout(&fx.agent, &fx.req(2, 500));
    m.request_payout(&fx.agent, &fx.req(3, 200));
    m.reject(&fx.approver, &fx.b32(2));
    let ids = Vec::from_array(
        &fx.env,
        [fx.b32(1), fx.b32(9), fx.b32(2), fx.b32(3), fx.b32(1)],
    );
    let out = m.cancel_batch(&fx.principal, &ids);
    let item = |id: u8, code: u32, status: Option<u32>| ItemResult {
        payout_id: fx.b32(id),
        code,
        status,
    };
    assert_eq!(
        out,
        Vec::from_array(
            &fx.env,
            [
                item(1, 0, Some(STATUS_CANCELLED)),
                item(9, MandateError::PayoutNotFound as u32, None),
                item(2, MandateError::PayoutWrongStatus as u32, None),
                item(3, 0, Some(STATUS_CANCELLED)),
                item(1, MandateError::PayoutWrongStatus as u32, None),
            ]
        )
    );
    // Refused items emit nothing.
    let ev = fx.events(&m.address);
    assert_eq!(ev.len(), 2);
    assert_eq!(m.state().committed, 0);
}

#[test]
fn cancel_batch_authority_is_per_item() {
    let fx = Fx::new();
    let m = fx.ready();
    let other = fx.account(21);
    fx.edit_roles(&m.address, |r| {
        r.agent_epoch += 1;
        r.agents.push_back(Agent {
            account: other.clone(),
            epoch: r.agent_epoch,
            pending: 0,
            committed: 0,
        });
    });
    m.request_payout(&fx.agent, &fx.req(1, 100));
    m.request_payout(&other, &fx.req(2, 100));
    let ids = Vec::from_array(&fx.env, [fx.b32(1), fx.b32(2)]);
    let out = m.cancel_batch(&fx.agent, &ids);
    assert_eq!(out.get(0).unwrap().code, 0);
    assert_eq!(out.get(1).unwrap().code, MandateError::Unauthorized as u32);
    assert_eq!(m.state().committed, 100);
}

#[test]
fn cancel_batch_bounds_after_revoked_check() {
    let fx = Fx::new();
    let m = fx.ready();
    let none: Vec<BytesN<32>> = Vec::new(&fx.env);
    fx.edit_flags(&m.address, |f| f.revoked = true);
    assert_eq!(
        code(m.try_cancel_batch(&fx.principal, &none)),
        MandateError::MandateRevoked
    );
    fx.edit_flags(&m.address, |f| f.revoked = false);
    assert_eq!(
        code(m.try_cancel_batch(&fx.principal, &none)),
        MandateError::BatchInvalid
    );
    let mut ids = Vec::new(&fx.env);
    for i in 1..=11u8 {
        m.request_payout(&fx.agent, &fx.req(i, 10));
        ids.push_back(fx.b32(i));
    }
    assert_eq!(
        code(m.try_cancel_batch(&fx.principal, &ids)),
        MandateError::BatchInvalid
    );
    ids.pop_back();
    let out = m.cancel_batch(&fx.principal, &ids);
    assert!(out.iter().all(|r| r.code == 0));
    assert_eq!(m.state().committed, 10);
}

#[test]
fn cancel_on_mandate_expiry_alone_needs_the_full_grace() {
    // No request can store a payout that outlives the mandate's grace, so
    // this one is written straight to storage to reach the rule on its own.
    let fx = Fx::new();
    let mut config = fx.config();
    config.limits.expires_at = NOW + 3_600;
    let m = fx.ready_with(&config);
    m.request_payout(&fx.agent, &fx.req(1, 100));
    let mut r = fx.record(&m, 1);
    r.expires_at = NOW + 3_600 + 2 * SWEEP_GRACE_SECONDS;
    fx.env
        .as_contract(&m.address, || crate::storage::update_record(&fx.env, &r));
    let stranger = fx.account(40);
    fx.set_time(NOW + 3_600 + SWEEP_GRACE_SECONDS - 1);
    assert_eq!(
        code(m.try_cancel(&stranger, &fx.b32(1))),
        MandateError::Unauthorized
    );
    fx.set_time(NOW + 3_600 + SWEEP_GRACE_SECONDS);
    m.cancel(&stranger, &fx.b32(1));
    cancelled(&fx, &m, 1, &stranger, CANCEL_LAPSED);
    assert_eq!(m.state().committed, 0);
}
