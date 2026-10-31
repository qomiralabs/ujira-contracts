// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The views and the values they derive on read.

use super::{Fx, DAY, NOW};
use crate::storage::{self, Flags, Totals};
use crate::types::{
    DailyUsage, PayoutEntry, PayoutRecord, Tombstone, STATUS_APPROVED, STATUS_EXECUTED,
};
use crate::SPEC_VERSION;
use soroban_sdk::testutils::{Ledger as _, MuxedAddress as MuxedAddressTestutils};
use soroban_sdk::{Address, MuxedAddress, Vec};

fn muxed(base: &Address, tag: Option<u64>) -> MuxedAddress {
    match tag {
        None => MuxedAddress::from(base),
        Some(id) => <MuxedAddress as MuxedAddressTestutils>::new(base, id),
    }
}

#[test]
fn version_is_the_spec_version() {
    let fx = Fx::new();
    assert_eq!(fx.mandate().version(), SPEC_VERSION);
    assert_eq!(SPEC_VERSION, 1);
}

#[test]
fn state_after_creation_holds_nothing() {
    let fx = Fx::new();
    let st = fx.mandate().state();
    assert_eq!((st.balance, st.free), (0, 0));
    assert!(!st.expired);
}

#[test]
fn plain_transfer_changes_free_without_a_call() {
    let fx = Fx::new();
    let m = fx.mandate();
    // Funding is a plain token transfer; the mandate is never called.
    fx.mint(&m.address, 700);
    let st = m.state();
    assert_eq!((st.balance, st.free), (700, 700));
    fx.mint(&m.address, 1);
    assert_eq!(m.state().free, 701);
}

#[test]
fn free_leaves_out_reservations_while_active() {
    let fx = Fx::new();
    let m = fx.mandate();
    fx.mint(&m.address, 1_000);
    fx.env.as_contract(&m.address, || {
        storage::set_totals(
            &fx.env,
            &Totals {
                spent: 5,
                pending: 300,
                committed: 200,
            },
        );
    });
    let st = m.state();
    assert_eq!((st.spent, st.pending, st.committed), (5, 300, 200));
    assert_eq!((st.balance, st.free), (1_000, 500));
    // Never below zero, even when the asset took funds back.
    fx.env.as_contract(&m.address, || {
        storage::set_totals(
            &fx.env,
            &Totals {
                spent: 0,
                pending: 900,
                committed: 900,
            },
        );
    });
    assert_eq!(m.state().free, 0);
}

#[test]
fn free_is_the_whole_balance_once_expired_or_revoked() {
    let fx = Fx::new();
    let m = fx.mandate();
    fx.mint(&m.address, 1_000);
    fx.env.as_contract(&m.address, || {
        storage::set_totals(
            &fx.env,
            &Totals {
                spent: 0,
                pending: 300,
                committed: 200,
            },
        );
    });
    let expires_at = m.config().limits.expires_at;

    // One second before expiry the mandate is active.
    fx.env.ledger().set_timestamp(expires_at - 1);
    let st = m.state();
    assert!(!st.expired);
    assert_eq!(st.free, 500);

    // At expiry it is expired.
    fx.env.ledger().set_timestamp(expires_at);
    let st = m.state();
    assert!(st.expired);
    assert_eq!(st.free, 1_000);

    // Revoked and not expired: also the whole balance.
    fx.env.ledger().set_timestamp(NOW);
    fx.env.as_contract(&m.address, || {
        storage::set_flags(
            &fx.env,
            &Flags {
                paused: false,
                revoked: true,
            },
        );
    });
    let st = m.state();
    assert!(st.revoked && !st.expired);
    assert_eq!(st.free, 1_000);
}

#[test]
fn config_reports_role_lists_in_insertion_order() {
    let fx = Fx::new();
    let mut config = fx.config();
    let later = fx.account(30);
    let first = fx.account(31);
    config.agents = fx.list(&[&later, &first, &fx.agent]);
    let id = fx.create(&config).unwrap();
    let view = crate::UjiraMandateClient::new(&fx.env, &id).config();
    assert_eq!(view.agents, fx.list(&[&later, &first, &fx.agent]));
}

#[test]
fn payout_answers_none_record_or_tombstone() {
    let fx = Fx::new();
    let m = fx.mandate();
    let id = fx.b32(1);
    assert_eq!(m.payout(&id), None);

    let record = PayoutRecord {
        payout_id: id.clone(),
        agent: fx.agent.clone(),
        agent_epoch: 0,
        recipient_ref: fx.b32(0xb0),
        total: 100,
        fee: 0,
        legs: Vec::from_array(
            &fx.env,
            [crate::types::Leg {
                destination: muxed(&fx.account(50), Some(9)),
                amount: 100,
            }],
        ),
        status: STATUS_APPROVED,
        created_at: NOW,
        expires_at: NOW + DAY,
        policy_epoch: 0,
        approvals: Vec::new(&fx.env),
    };
    fx.env
        .as_contract(&m.address, || storage::put_new_record(&fx.env, &record));
    assert_eq!(m.payout(&id), Some(PayoutEntry::Record(record)));

    fx.env.as_contract(&m.address, || {
        storage::put_tombstone(&fx.env, &id, STATUS_EXECUTED)
    });
    assert_eq!(
        m.payout(&id),
        Some(PayoutEntry::Tombstone(Tombstone {
            payout_id: id.clone(),
            status: STATUS_EXECUTED,
        }))
    );
}

#[test]
fn daily_used_counts_only_today() {
    let fx = Fx::new();
    let m = fx.mandate();
    let bob = fx.b32(0xb0);
    let today = NOW / DAY;
    // Never used: nothing today.
    assert_eq!(
        m.daily_used(&bob),
        DailyUsage {
            day: today,
            used: 0
        }
    );

    fx.env.as_contract(&m.address, || {
        storage::set_daily(
            &fx.env,
            &bob,
            &DailyUsage {
                day: today,
                used: 400,
            },
        );
    });
    assert_eq!(
        m.daily_used(&bob),
        DailyUsage {
            day: today,
            used: 400
        }
    );
    // Last second of the day, then the first of the next.
    fx.env.ledger().set_timestamp((today + 1) * DAY - 1);
    assert_eq!(m.daily_used(&bob).used, 400);
    fx.env.ledger().set_timestamp((today + 1) * DAY);
    assert_eq!(
        m.daily_used(&bob),
        DailyUsage {
            day: today + 1,
            used: 0
        }
    );
}

#[test]
fn is_allowed_matches_the_recipient_address_and_tag() {
    let fx = Fx::new();
    let m = fx.mandate();
    let bob = fx.b32(0xb0);
    let carol = fx.b32(0xca);
    let exchange = fx.account(50);
    let plain = muxed(&exchange, None);
    let tag_zero = muxed(&exchange, Some(0));
    let tag_seven = muxed(&exchange, Some(7));
    assert!(!m.is_allowed(&bob, &plain));

    fx.env.as_contract(&m.address, || {
        storage::set_allowed(&fx.env, &bob, &tag_seven);
    });
    assert!(m.is_allowed(&bob, &tag_seven));
    // Another tag, no tag, or another recipient is a different pair.
    assert!(!m.is_allowed(&bob, &tag_zero));
    assert!(!m.is_allowed(&bob, &plain));
    assert!(!m.is_allowed(&carol, &tag_seven));

    fx.env.as_contract(&m.address, || {
        storage::set_allowed(&fx.env, &bob, &plain);
        storage::remove_allowed(&fx.env, &bob, &tag_seven);
    });
    assert!(m.is_allowed(&bob, &plain));
    assert!(!m.is_allowed(&bob, &tag_seven));
}
