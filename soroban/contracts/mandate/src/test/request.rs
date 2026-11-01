// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! `request_payout` and `request_payouts`: the check order, the effects,
//! the record's lifetime, and the batch running per item.

extern crate std;

use super::{code, dest, Fx, BOB_WALLET, DAY, FUNDED, MIN_PERSISTENT_TTL, NOW};
use crate::events::{PayoutRequestRejected, PayoutRequested};
use crate::payout::fee_cap;
use crate::storage::{
    self, Agent, DataKey, Totals, RECORD_TTL_MARGIN_SECONDS, STELLAR_LEDGER_SECONDS,
};
use crate::types::{
    Attestation, DailyUsage, ItemResult, Leg, PayoutRequest, STATUS_APPROVED, STATUS_CANCELLED,
    STATUS_PENDING,
};
use crate::{MandateError, UjiraMandateClient};
use soroban_sdk::testutils::storage::Persistent as _;
use soroban_sdk::testutils::Ledger as _;
use soroban_sdk::{Bytes, BytesN, Event as _, MuxedAddress, Vec};

const I128_MAX: i128 = i128::MAX;

fn leg(d: MuxedAddress, amount: i128) -> Leg {
    Leg {
        destination: d,
        amount,
    }
}

fn legs(fx: &Fx, items: &[(MuxedAddress, i128)]) -> Vec<Leg> {
    let mut out = Vec::new(&fx.env);
    for (d, a) in items {
        out.push_back(leg(d.clone(), *a));
    }
    out
}

fn refused(fx: &Fx, m: &UjiraMandateClient, req: &PayoutRequest) -> MandateError {
    code(m.try_request_payout(&fx.agent, req))
}

fn set_totals(fx: &Fx, m: &UjiraMandateClient, t: Totals) {
    fx.env
        .as_contract(&m.address, || storage::set_totals(&fx.env, &t));
}

#[test]
fn request_below_threshold_is_approved_and_committed() {
    let fx = Fx::new();
    let m = fx.ready();
    let req = fx.req(1, 400);
    assert_eq!(m.request_payout(&fx.agent, &req), STATUS_APPROVED);
    assert_eq!(
        fx.events(&m.address),
        [PayoutRequested {
            payout_id: req.payout_id.clone(),
            agent: fx.agent.clone(),
            recipient_ref: fx.bob(),
            total: 400,
            fee: 0,
            legs: req.legs.clone(),
            task_ref: req.task_ref.clone(),
            status: STATUS_APPROVED,
            expires_at: NOW + DAY,
        }
        .to_xdr(&fx.env, &m.address)]
    );
    let st = m.state();
    assert_eq!((st.spent, st.pending, st.committed), (0, 0, 400));
    assert_eq!((st.balance, st.free), (FUNDED, FUNDED - 400));
    let r = fx.record(&m, 1);
    assert_eq!(r.status, STATUS_APPROVED);
    assert_eq!(r.agent, fx.agent);
    assert_eq!(r.agent_epoch, 0);
    assert_eq!((r.total, r.fee), (400, 0));
    assert_eq!((r.created_at, r.expires_at), (NOW, NOW + DAY));
    assert_eq!(r.policy_epoch, 0);
    assert!(r.approvals.is_empty());
}

#[test]
fn request_at_threshold_is_pending() {
    let fx = Fx::new();
    let m = fx.ready();
    assert_eq!(m.request_payout(&fx.agent, &fx.req(1, 500)), STATUS_PENDING);
    let st = m.state();
    assert_eq!((st.pending, st.committed, st.free), (500, 0, FUNDED - 500));
}

#[test]
fn request_charge_including_fee_decides_threshold_and_reservation() {
    let fx = Fx::new();
    let mut config = fx.config();
    config.max_fee_bps = 100;
    let m = fx.ready_with(&config);
    // 496 + 4 reaches the threshold of 500 although the total does not.
    let mut req = fx.req(1, 496);
    req.fee = 4;
    assert_eq!(m.request_payout(&fx.agent, &req), STATUS_PENDING);
    assert_eq!(m.state().pending, 500);
    // One unit more than the cap allows.
    let mut over = fx.req(2, 400);
    over.fee = 5;
    assert_eq!(refused(&fx, &m, &over), MandateError::FeeExceedsCap);
}

#[test]
fn fee_cap_rounds_down_and_never_overflows() {
    assert_eq!(fee_cap(9_999, 1), Ok(0));
    assert_eq!(fee_cap(10_000, 1), Ok(1));
    assert_eq!(fee_cap(19_999, 1), Ok(1));
    assert_eq!(fee_cap(500, 100), Ok(5));
    assert_eq!(fee_cap(1_000, 0), Ok(0));
    // floor(I128_MAX * 100 / 10000) is floor(I128_MAX / 100).
    assert_eq!(fee_cap(I128_MAX, 100), Ok(I128_MAX / 100));
    assert_eq!(fee_cap(I128_MAX, 1), Ok(I128_MAX / 10_000));
}

#[test]
fn request_caller_and_active_checks_come_first() {
    let fx = Fx::new();
    let m = fx.ready();
    // Every later check would fail too: unknown recipient, no deadline.
    let mut bad = fx.req(1, 0);
    bad.deadline = 0;
    // Not an agent.
    for who in [&fx.principal, &fx.approver, &fx.executor, &fx.guardian] {
        assert_eq!(
            code(m.try_request_payout(who, &bad)),
            MandateError::Unauthorized
        );
    }
    // Revoked, then expired, then paused.
    fx.edit_flags(&m.address, |f| {
        f.paused = true;
        f.revoked = true;
    });
    fx.set_time(NOW + 366 * DAY);
    assert_eq!(refused(&fx, &m, &bad), MandateError::MandateRevoked);
    fx.edit_flags(&m.address, |f| f.revoked = false);
    assert_eq!(refused(&fx, &m, &bad), MandateError::MandateExpired);
    fx.set_time(NOW);
    assert_eq!(refused(&fx, &m, &bad), MandateError::MandatePaused);
    fx.edit_flags(&m.address, |f| f.paused = false);
    assert_eq!(refused(&fx, &m, &bad), MandateError::RequestDeadlinePassed);
}

#[test]
fn request_id_then_deadline_then_amounts() {
    let fx = Fx::new();
    let m = fx.ready();
    m.request_payout(&fx.agent, &fx.req(1, 100));
    // A used ID is reported before anything else about the request.
    let mut again = fx.req(1, 0);
    again.deadline = NOW;
    assert_eq!(refused(&fx, &m, &again), MandateError::PayoutIdExists);
    // The deadline is reached at the deadline itself.
    let mut late = fx.req(2, 0);
    late.deadline = NOW;
    assert_eq!(refused(&fx, &m, &late), MandateError::RequestDeadlinePassed);
    late.deadline = NOW + 1;
    assert_eq!(refused(&fx, &m, &late), MandateError::AmountInvalid);
}

#[test]
fn request_amount_domain() {
    let fx = Fx::new();
    let m = fx.ready();
    let mut req = fx.req(1, 100);
    for (total, fee) in [
        (0, 0),
        (-1, 0),
        (i128::MIN, 0),
        (100, -1),
        (100, i128::MIN),
        (I128_MAX, 1),
        (1, I128_MAX),
        (I128_MAX, I128_MAX),
    ] {
        req.total = total;
        req.fee = fee;
        assert_eq!(
            refused(&fx, &m, &req),
            MandateError::AmountInvalid,
            "total {total} fee {fee}"
        );
    }
    // The largest charge that fits passes the amount checks and is stopped
    // by the leg sum instead.
    req.total = I128_MAX - 5;
    req.fee = 5;
    assert_eq!(refused(&fx, &m, &req), MandateError::LegsSumMismatch);
}

#[test]
fn request_leg_rules() {
    let fx = Fx::new();
    let m = fx.ready();
    let bob = fx.bob_wallet();
    let carol = dest(&fx.env, 51, None);
    let mut req = fx.req(1, 100);
    let cases: [(std::vec::Vec<(MuxedAddress, i128)>, MandateError); 9] = [
        (std::vec![], MandateError::LegsInvalid),
        (
            std::vec![
                (bob.clone(), 20),
                (dest(&fx.env, 51, None), 20),
                (dest(&fx.env, 52, None), 20),
                (dest(&fx.env, 53, None), 20),
                (dest(&fx.env, 54, None), 20),
            ],
            MandateError::LegsInvalid,
        ),
        (
            std::vec![(bob.clone(), 100), (carol.clone(), 0)],
            MandateError::LegsInvalid,
        ),
        (
            std::vec![(bob.clone(), 101), (carol.clone(), -1)],
            MandateError::LegsInvalid,
        ),
        // The fee address, under any tag, is never a leg.
        (
            std::vec![(MuxedAddress::from(fx.fee_address.clone()), 100)],
            MandateError::LegsInvalid,
        ),
        (
            std::vec![(bob.clone(), 50), (bob.clone(), 50)],
            MandateError::LegsInvalid,
        ),
        (std::vec![(bob.clone(), 99)], MandateError::LegsSumMismatch),
        (std::vec![(bob.clone(), 101)], MandateError::LegsSumMismatch),
        // A leg past the total fails at that leg, before any overflow.
        (
            std::vec![(bob.clone(), 60), (carol.clone(), I128_MAX)],
            MandateError::LegsSumMismatch,
        ),
    ];
    for (i, (l, want)) in cases.into_iter().enumerate() {
        req.legs = legs(&fx, &l);
        assert_eq!(refused(&fx, &m, &req), want, "case {i}");
    }
    let tagged_fee =
        <MuxedAddress as soroban_sdk::testutils::MuxedAddress>::new(&fx.fee_address, 9);
    req.legs = legs(&fx, &[(tagged_fee, 100)]);
    assert_eq!(refused(&fx, &m, &req), MandateError::LegsInvalid);
}

#[test]
fn request_same_address_under_two_tags_is_two_destinations() {
    let fx = Fx::new();
    let m = fx.ready();
    let t1 = dest(&fx.env, BOB_WALLET, Some(1));
    let t2 = dest(&fx.env, BOB_WALLET, Some(2));
    let t0 = dest(&fx.env, BOB_WALLET, Some(0));
    for d in [&t1, &t2] {
        fx.allow(&m.address, &fx.bob(), d);
    }
    let mut req = fx.req(1, 100);
    req.legs = legs(&fx, &[(t1.clone(), 60), (t2.clone(), 40)]);
    assert_eq!(m.request_payout(&fx.agent, &req), STATUS_APPROVED);
    // Tag 0 is not the same pair as no tag, and was never allowed.
    let mut zero = fx.req(2, 100);
    zero.legs = legs(&fx, &[(t0, 100)]);
    assert_eq!(refused(&fx, &m, &zero), MandateError::RecipientNotAllowed);
    // Allowed for Bob is not allowed for anyone else.
    let mut other = fx.req(3, 100);
    other.recipient_ref = fx.b32(0xc1);
    assert_eq!(refused(&fx, &m, &other), MandateError::RecipientNotAllowed);
}

#[test]
fn request_allowlist_mode_takes_no_attestations() {
    let fx = Fx::new();
    let m = fx.ready();
    let mut req = fx.req(1, 100);
    req.attestations.push_back(Attestation {
        valid_until: NOW + 100,
        policy_epoch: 0,
        signature: Bytes::from_array(&fx.env, &[1; 64]),
    });
    assert_eq!(refused(&fx, &m, &req), MandateError::AttestationInvalid);
    // The attestation rule comes after the leg rules.
    req.legs = Vec::new(&fx.env);
    assert_eq!(refused(&fx, &m, &req), MandateError::LegsInvalid);
}

#[test]
fn request_limits_in_order() {
    let fx = Fx::new();
    let mut config = fx.config();
    config.max_fee_bps = 100;
    let m = fx.ready_with(&config);
    // The fee cap comes before the per-payment cap.
    let mut req = fx.req(1, 2_000);
    req.fee = 21;
    assert_eq!(refused(&fx, &m, &req), MandateError::FeeExceedsCap);
    // The per-payment cap is on the charge: 1,000 plus a fee of 1.
    let mut req = fx.req(1, 1_000);
    req.fee = 1;
    assert_eq!(refused(&fx, &m, &req), MandateError::PaymentCapExceeded);
    req.fee = 0;
    // The budget: 10,000 less 9,001 reserved or spent leaves 999.
    set_totals(
        &fx,
        &m,
        Totals {
            spent: 9_000,
            pending: 1,
            committed: 0,
        },
    );
    assert_eq!(refused(&fx, &m, &req), MandateError::BudgetExceeded);
    set_totals(
        &fx,
        &m,
        Totals {
            spent: 0,
            pending: 0,
            committed: 0,
        },
    );
    // The daily cap of 500 is on the total; executed usage today counts and
    // a stored earlier day does not.
    let today = NOW / DAY;
    let mut small = fx.req(2, 101);
    fx.env.as_contract(&m.address, || {
        storage::set_daily(
            &fx.env,
            &fx.bob(),
            &DailyUsage {
                day: today,
                used: 400,
            },
        )
    });
    assert_eq!(refused(&fx, &m, &small), MandateError::DailyCapExceeded);
    small.total = 100;
    small.legs = legs(&fx, &[(fx.bob_wallet(), 100)]);
    small.fee = 1;
    assert_eq!(m.request_payout(&fx.agent, &small), STATUS_APPROVED);
    fx.env.as_contract(&m.address, || {
        storage::set_daily(
            &fx.env,
            &fx.bob(),
            &DailyUsage {
                day: today - 1,
                used: 500,
            },
        )
    });
    let full = fx.req(3, 500);
    assert_eq!(m.request_payout(&fx.agent, &full), STATUS_PENDING);
    // Over the daily cap even with nothing used.
    let mut over = fx.req(4, 501);
    over.legs = legs(&fx, &[(fx.bob_wallet(), 501)]);
    assert_eq!(refused(&fx, &m, &over), MandateError::DailyCapExceeded);
}

#[test]
fn request_needs_free_funds_for_the_charge() {
    let fx = Fx::new();
    let mut config = fx.config();
    config.max_fee_bps = 100;
    let id = fx.create(&config).unwrap();
    let m = UjiraMandateClient::new(&fx.env, &id);
    fx.allow(&id, &fx.bob(), &fx.bob_wallet());
    assert_eq!(
        refused(&fx, &m, &fx.req(1, 100)),
        MandateError::InsufficientFunds
    );
    fx.mint(&id, 400);
    m.request_payout(&fx.agent, &fx.req(1, 300));
    // 100 free; the fee counts toward the charge.
    let mut req = fx.req(2, 100);
    req.fee = 1;
    assert_eq!(refused(&fx, &m, &req), MandateError::InsufficientFunds);
    req.fee = 0;
    assert_eq!(m.request_payout(&fx.agent, &req), STATUS_APPROVED);
    assert_eq!(m.state().free, 0);
}

#[test]
fn refused_request_writes_and_emits_nothing() {
    let fx = Fx::new();
    let m = fx.ready();
    let before = m.state();
    let mut req = fx.req(1, 100);
    req.recipient_ref = fx.b32(0xc1);
    assert_eq!(refused(&fx, &m, &req), MandateError::RecipientNotAllowed);
    assert!(fx.events(&m.address).is_empty());
    assert_eq!(m.state(), before);
    assert_eq!(m.payout(&fx.b32(1)), None);
}

#[test]
fn request_payouts_run_per_item_against_earlier_items() {
    let fx = Fx::new();
    let m = fx.ready();
    let mut reqs = Vec::new(&fx.env);
    reqs.push_back(fx.req(1, 300));
    // The same ID again, later in the same batch.
    reqs.push_back(fx.req(1, 100));
    let mut bad = fx.req(2, 100);
    bad.recipient_ref = fx.b32(0xc1);
    reqs.push_back(bad);
    reqs.push_back(fx.req(3, 500));
    let out = m.request_payouts(&fx.agent, &reqs);
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
                item(1, 0, Some(STATUS_APPROVED)),
                item(1, MandateError::PayoutIdExists as u32, None),
                item(2, MandateError::RecipientNotAllowed as u32, None),
                item(3, 0, Some(STATUS_PENDING)),
            ]
        )
    );
    let ev = fx.events(&m.address);
    assert_eq!(ev.len(), 4);
    assert_eq!(
        ev[1],
        PayoutRequestRejected {
            payout_id: fx.b32(1),
            agent: fx.agent.clone(),
            code: MandateError::PayoutIdExists as u32,
        }
        .to_xdr(&fx.env, &m.address)
    );
    assert_eq!(
        ev[2],
        PayoutRequestRejected {
            payout_id: fx.b32(2),
            agent: fx.agent.clone(),
            code: MandateError::RecipientNotAllowed as u32,
        }
        .to_xdr(&fx.env, &m.address)
    );
    let st = m.state();
    assert_eq!((st.pending, st.committed), (500, 300));
    assert_eq!(fx.record(&m, 1).total, 300);
    assert_eq!(m.payout(&fx.b32(2)), None);
}

#[test]
fn request_payouts_later_item_sees_budget_used_by_earlier_ones() {
    let fx = Fx::new();
    let mut config = fx.config();
    config.limits.total_budget = 1_000;
    let m = fx.ready_with(&config);
    let mut reqs = Vec::new(&fx.env);
    reqs.push_back(fx.req(1, 400));
    reqs.push_back(fx.req(2, 400));
    reqs.push_back(fx.req(3, 400));
    let out = m.request_payouts(&fx.agent, &reqs);
    assert_eq!(
        out.get(2).unwrap().code,
        MandateError::BudgetExceeded as u32
    );
    assert_eq!(m.state().committed, 800);
}

#[test]
fn request_payouts_length_after_caller_and_active_checks() {
    let fx = Fx::new();
    let m = fx.ready();
    let empty: Vec<PayoutRequest> = Vec::new(&fx.env);
    assert_eq!(
        code(m.try_request_payouts(&fx.approver, &empty)),
        MandateError::Unauthorized
    );
    fx.edit_flags(&m.address, |f| f.paused = true);
    assert_eq!(
        code(m.try_request_payouts(&fx.agent, &empty)),
        MandateError::MandatePaused
    );
    fx.edit_flags(&m.address, |f| f.paused = false);
    assert_eq!(
        code(m.try_request_payouts(&fx.agent, &empty)),
        MandateError::BatchInvalid
    );
    let mut eleven = Vec::new(&fx.env);
    for i in 0..11 {
        eleven.push_back(fx.req(i + 1, 10));
    }
    assert_eq!(
        code(m.try_request_payouts(&fx.agent, &eleven)),
        MandateError::BatchInvalid
    );
    assert!(fx.events(&m.address).is_empty());
    eleven.pop_back();
    let out = m.request_payouts(&fx.agent, &eleven);
    assert!(out.iter().all(|r| r.code == 0));
    assert_eq!(m.state().committed, 100);
}

#[test]
fn extreme_inputs_return_errors_not_panics() {
    let fx = Fx::new();
    let mut config = fx.config();
    config.limits.total_budget = I128_MAX;
    config.limits.per_payment_cap = I128_MAX;
    config.limits.per_recipient_daily_cap = I128_MAX;
    config.limits.approval_threshold = I128_MAX;
    config.max_fee_bps = 100;
    let m = fx.ready_with(&config);
    // The largest amounts, and every amount past them.
    let mut req = fx.req(1, I128_MAX);
    req.fee = I128_MAX;
    assert_eq!(refused(&fx, &m, &req), MandateError::AmountInvalid);
    req.fee = 1;
    assert_eq!(refused(&fx, &m, &req), MandateError::AmountInvalid);
    req.fee = 0;
    // Passes every amount, leg and limit check up to the funds.
    assert_eq!(refused(&fx, &m, &req), MandateError::InsufficientFunds);
    // A fee at its cap on the largest total is still only the funds short.
    let mut big = fx.req(2, I128_MAX / 2);
    big.fee = fee_cap(I128_MAX / 2, 100).unwrap();
    assert_eq!(refused(&fx, &m, &big), MandateError::InsufficientFunds);
    // Legs at the largest amount.
    let mut two = fx.req(3, I128_MAX);
    two.legs = legs(
        &fx,
        &[
            (fx.bob_wallet(), I128_MAX),
            (dest(&fx.env, 51, None), I128_MAX),
        ],
    );
    assert_eq!(refused(&fx, &m, &two), MandateError::LegsSumMismatch);
    // Empty and oversized lists.
    let mut empty = fx.req(4, 1);
    empty.legs = Vec::new(&fx.env);
    assert_eq!(refused(&fx, &m, &empty), MandateError::LegsInvalid);
    let mut many = fx.req(5, 1);
    for _ in 0..40 {
        many.legs.push_back(leg(fx.bob_wallet(), I128_MAX));
    }
    assert_eq!(refused(&fx, &m, &many), MandateError::LegsInvalid);
    let mut atts = fx.req(6, 1);
    for _ in 0..40 {
        atts.attestations.push_back(Attestation {
            valid_until: u64::MAX,
            policy_epoch: u64::MAX,
            signature: Bytes::from_array(&fx.env, &[0xff; 200]),
        });
    }
    assert_eq!(refused(&fx, &m, &atts), MandateError::AttestationInvalid);
    // Times at their extremes.
    let mut late = fx.req(7, 1);
    late.deadline = u64::MAX;
    assert_eq!(m.request_payout(&fx.agent, &late), STATUS_APPROVED);
    let mut past = fx.req(8, 1);
    past.deadline = 0;
    assert_eq!(refused(&fx, &m, &past), MandateError::RequestDeadlinePassed);
    // Batches and cancels at and past their bounds.
    let none: Vec<BytesN<32>> = Vec::new(&fx.env);
    assert_eq!(
        code(m.try_cancel_batch(&fx.principal, &none)),
        MandateError::BatchInvalid
    );
    let mut ids = Vec::new(&fx.env);
    for i in 0..200u8 {
        ids.push_back(fx.b32(i));
    }
    assert_eq!(
        code(m.try_cancel_batch(&fx.principal, &ids)),
        MandateError::BatchInvalid
    );
    let mut reqs = Vec::new(&fx.env);
    for i in 0..50u8 {
        reqs.push_back(fx.req(100 + i, I128_MAX));
    }
    assert_eq!(
        code(m.try_request_payouts(&fx.agent, &reqs)),
        MandateError::BatchInvalid
    );
    // Unknown IDs everywhere a payout is looked up.
    let ghost = fx.b32(0xee);
    assert_eq!(
        code(m.try_approve(&fx.approver, &ghost)),
        MandateError::PayoutNotFound
    );
    assert_eq!(
        code(m.try_reject(&fx.approver, &ghost)),
        MandateError::PayoutNotFound
    );
    assert_eq!(
        code(m.try_cancel(&fx.principal, &ghost)),
        MandateError::PayoutNotFound
    );
}

// The request-driven versions of the storage lifetime tests.

#[test]
fn request_reusing_archived_id_gives_payout_id_exists() {
    let fx = Fx::new();
    let m = fx.ready();
    m.request_payout(&fx.agent, &fx.req(1, 100));
    m.request_payout(&fx.agent, &fx.req(2, 100));
    m.cancel(&fx.principal, &fx.b32(2));
    assert_eq!(fx.tombstone(&m, 2).status, STATUS_CANCELLED);
    let last = fx.env.as_contract(&m.address, || {
        fx.env
            .storage()
            .persistent()
            .get_ttl(&DataKey::Payout(fx.b32(1)))
    }) + fx.env.ledger().sequence();
    // Past the record's and the tombstone's last live ledger; the host
    // restores an archived entry on read, as a simulated transaction does.
    let seconds = u64::from(last + 10 - fx.env.ledger().sequence()) * STELLAR_LEDGER_SECONDS;
    fx.env.ledger().with_mut(|li| {
        li.sequence_number = last + 10;
        li.timestamp += seconds;
    });
    for id in [1, 2] {
        assert_eq!(
            refused(&fx, &m, &fx.req(id, 100)),
            MandateError::PayoutIdExists,
            "payout {id}"
        );
    }
}

#[test]
fn request_record_ttl_reaches_expires_at_plus_margin() {
    let fx = Fx::new();
    let mut config = fx.config();
    config.limits.payout_ttl_seconds = 30 * DAY;
    let m = fx.ready_with(&config);
    m.request_payout(&fx.agent, &fx.req(1, 100));
    let r = fx.record(&m, 1);
    assert_eq!(r.expires_at, NOW + 30 * DAY);
    let ttl = fx.env.as_contract(&m.address, || {
        fx.env
            .storage()
            .persistent()
            .get_ttl(&DataKey::Payout(fx.b32(1)))
    });
    let seconds = r.expires_at + RECORD_TTL_MARGIN_SECONDS - NOW;
    assert_eq!(u64::from(ttl), seconds.div_ceil(STELLAR_LEDGER_SECONDS));
}

#[test]
fn request_record_ttl_never_below_network_minimum() {
    let fx = Fx::new();
    let mut config = fx.config();
    config.limits.payout_ttl_seconds = 3_600;
    let m = fx.ready_with(&config);
    m.request_payout(&fx.agent, &fx.req(1, 100));
    let ttl = fx.env.as_contract(&m.address, || {
        fx.env
            .storage()
            .persistent()
            .get_ttl(&DataKey::Payout(fx.b32(1)))
    });
    assert_eq!(ttl, MIN_PERSISTENT_TTL - 1);
    assert!(u64::from(ttl) * STELLAR_LEDGER_SECONDS >= 3_600 + RECORD_TTL_MARGIN_SECONDS);
}

#[test]
fn request_near_the_largest_time_caps_the_expiry() {
    let fx = Fx::new();
    let mut config = fx.config();
    config.limits.payout_ttl_seconds = 3_600;
    config.limits.expires_at = u64::MAX;
    let m = fx.ready_with(&config);
    let at = |id: u8, now: u64| {
        fx.set_time(now);
        let mut req = fx.req(id, 100);
        req.deadline = u64::MAX;
        assert_eq!(m.request_payout(&fx.agent, &req), STATUS_APPROVED);
        let r = fx.record(&m, id);
        assert_eq!(r.created_at, now);
        r.expires_at
    };
    // The sum fits up to the last second, and only past it is capped.
    assert_eq!(at(1, u64::MAX - 3_601), u64::MAX - 1);
    assert_eq!(at(2, u64::MAX - 3_600), u64::MAX);
    assert_eq!(at(3, u64::MAX - 1_800), u64::MAX);

    // A wrapped expiry would have let anyone cancel at once; the capped one
    // lapses only at the largest time.
    let stranger = fx.account(40);
    fx.set_time(u64::MAX - 1);
    assert_eq!(
        code(m.try_cancel(&stranger, &fx.b32(3))),
        MandateError::Unauthorized
    );
    fx.set_time(u64::MAX);
    m.cancel(&stranger, &fx.b32(3));
    assert_eq!(fx.tombstone(&m, 3).status, STATUS_CANCELLED);
}

#[test]
fn request_stores_the_agents_epoch_on_the_payout() {
    let fx = Fx::new();
    let m = fx.ready();
    let later = fx.account(21);
    // An agent added after creation holds a later epoch, as add_role will
    // give it.
    fx.edit_roles(&m.address, |r| {
        r.agent_epoch = 3;
        r.agents.push_back(Agent {
            account: later.clone(),
            epoch: 3,
        });
    });
    m.request_payout(&fx.agent, &fx.req(1, 100));
    m.request_payout(&later, &fx.req(2, 100));
    assert_eq!(fx.record(&m, 1).agent_epoch, 0);
    let r = fx.record(&m, 2);
    assert_eq!((r.agent, r.agent_epoch), (later, 3));
}
