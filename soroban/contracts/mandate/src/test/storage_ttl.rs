// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Storage kinds and TTLs: which kind each entry uses, how long a payout
//! record lives, and that an archived record or tombstone still blocks its
//! ID. Records are written with the storage functions the request and its
//! terminal transitions use, inside the deployed wasm contract's storage.

use super::{Fx, DAY, MIN_PERSISTENT_TTL, NOW, START_SEQUENCE};
use crate::storage::{
    self, DataKey, INSTANCE_TTL_EXTEND_TO, RECORD_TTL_MARGIN_SECONDS, STELLAR_LEDGER_SECONDS,
};
use crate::types::{DailyUsage, Leg, PayoutEntry, PayoutRecord, STATUS_CANCELLED, STATUS_PENDING};
use crate::{MandateError, UjiraMandateClient};
use soroban_sdk::testutils::storage::{Instance as _, Persistent as _};
use soroban_sdk::testutils::Ledger as _;
use soroban_sdk::xdr::{ContractDataDurability, LedgerKey, ScAddress, ScVal};
use soroban_sdk::{Address, BytesN, MuxedAddress, Vec};

fn record(fx: &Fx, id: &BytesN<32>, expires_at: u64) -> PayoutRecord {
    PayoutRecord {
        payout_id: id.clone(),
        agent: fx.agent.clone(),
        agent_epoch: 0,
        recipient_ref: fx.b32(0xb0),
        total: 100,
        fee: 0,
        legs: Vec::from_array(
            &fx.env,
            [Leg {
                destination: MuxedAddress::from(fx.account(50)),
                amount: 100,
            }],
        ),
        status: STATUS_PENDING,
        created_at: fx.env.ledger().timestamp(),
        expires_at,
        policy_epoch: 0,
        approvals: Vec::new(&fx.env),
    }
}

fn put(fx: &Fx, m: &Address, r: &PayoutRecord) {
    fx.env
        .as_contract(m, || storage::put_new_record(&fx.env, r));
}

/// The record's TTL in ledgers: its last live ledger minus the current one.
fn ttl(fx: &Fx, m: &Address, id: &BytesN<32>) -> u32 {
    fx.env.as_contract(m, || {
        fx.env
            .storage()
            .persistent()
            .get_ttl(&DataKey::Payout(id.clone()))
    })
}

fn live_until(fx: &Fx, m: &Address, id: &BytesN<32>) -> u32 {
    fx.env.ledger().sequence() + ttl(fx, m, id)
}

/// Moves time and the ledger sequence on together, one ledger per five
/// seconds, as a network does.
fn advance(fx: &Fx, seconds: u64) {
    let ledgers = u32::try_from(seconds / STELLAR_LEDGER_SECONDS).unwrap();
    fx.env.ledger().with_mut(|li| {
        li.timestamp += seconds;
        li.sequence_number += ledgers;
    });
}

#[test]
fn storage_types_are_instance_or_persistent() {
    let fx = Fx::new();
    let m = fx.mandate();
    let id = fx.b32(1);
    let bob = fx.b32(0xb0);
    put(&fx, &m.address, &record(&fx, &id, NOW + DAY));
    fx.env.as_contract(&m.address, || {
        storage::set_allowed(&fx.env, &bob, &MuxedAddress::from(fx.account(50)));
        storage::set_daily(&fx.env, &bob, &DailyUsage { day: 1, used: 1 });
    });

    // Every entry the mandate owns, of every kind, read from the ledger.
    let mut instance = 0;
    let mut persistent = 0;
    let me = ScAddress::from(&m.address);
    for (key, _) in fx.env.host().get_stored_entries().unwrap() {
        let LedgerKey::ContractData(k) = key.as_ref() else {
            continue;
        };
        if k.contract != me {
            continue;
        }
        assert_eq!(
            k.durability,
            ContractDataDurability::Persistent,
            "the mandate wrote a temporary entry: {:?}",
            k.key
        );
        if k.key == ScVal::LedgerKeyContractInstance {
            instance += 1;
        } else {
            persistent += 1;
        }
    }
    assert_eq!(instance, 1);
    // The record, the allowlist entry and the day usage.
    assert_eq!(persistent, 3);
    // The configuration and counters are all in the instance.
    let keys = fx
        .env
        .as_contract(&m.address, || fx.env.storage().instance().all().len());
    // Fixed, principal, limits, policy, roles, totals and flags.
    assert_eq!(keys, 7);
    let rule_entries = fx
        .env
        .as_contract(&m.address, || fx.env.storage().persistent().all().len());
    assert_eq!(rule_entries, 3);
}

#[test]
fn record_live_until_reaches_expires_at_plus_margin() {
    // A long payout: the record is extended past the network minimum.
    let fx = Fx::new();
    let m = fx.mandate();
    let id = fx.b32(1);
    let expires_at = NOW + 30 * DAY;
    put(&fx, &m.address, &record(&fx, &id, expires_at));
    let seconds = expires_at + RECORD_TTL_MARGIN_SECONDS - NOW;
    let t = ttl(&fx, &m.address, &id);
    assert_eq!(u64::from(t), seconds / STELLAR_LEDGER_SECONDS);
    assert_eq!(live_until(&fx, &m.address, &id), START_SEQUENCE + t);
    // At five seconds a ledger it is live through the last second of the
    // margin.
    assert!(u64::from(t) * STELLAR_LEDGER_SECONDS >= seconds);

    // A short payout: the network minimum already covers the margin, and the
    // extension never shortens it.
    let short = fx.b32(2);
    put(&fx, &m.address, &record(&fx, &short, NOW + 3_600));
    let t = ttl(&fx, &m.address, &short);
    assert!(u64::from(t) * STELLAR_LEDGER_SECONDS >= 3_600 + RECORD_TTL_MARGIN_SECONDS);
    assert_eq!(t, MIN_PERSISTENT_TTL - 1);
}

#[test]
fn record_ttl_uses_ceil_of_seconds_over_ledger_seconds() {
    let fx = Fx::new();
    let m = fx.mandate();
    let id = fx.b32(1);
    // 30 days less 2 seconds plus the margin: 2,678,398 seconds, which is
    // 535,679.6 ledgers. Rounding down would archive the record 3 seconds
    // early.
    let expires_at = NOW + 30 * DAY - 2;
    put(&fx, &m.address, &record(&fx, &id, expires_at));
    assert_eq!(ttl(&fx, &m.address, &id), 535_680);
    assert_eq!(
        storage::record_ttl_ledgers(NOW, expires_at, u32::MAX),
        535_680
    );
    assert_eq!(
        storage::record_ttl_ledgers(NOW, NOW + 30 * DAY, u32::MAX),
        535_680
    );
    assert_eq!(
        storage::record_ttl_ledgers(NOW, NOW + 30 * DAY + 1, u32::MAX),
        535_681
    );
}

#[test]
fn record_ttl_clamped_to_max_entry_ttl() {
    let fx = Fx::new();
    let m = fx.mandate();
    // A network maximum below what the record would need.
    let max_entry_ttl = 200_000;
    fx.env
        .ledger()
        .with_mut(|li| li.max_entry_ttl = max_entry_ttl);
    let needed = storage::record_ttl_ledgers(NOW, NOW + 30 * DAY, u32::MAX);
    assert!(needed > max_entry_ttl);
    let id = fx.b32(1);
    put(&fx, &m.address, &record(&fx, &id, NOW + 30 * DAY));
    assert_eq!(
        live_until(&fx, &m.address, &id),
        fx.env.ledger().max_live_until_ledger()
    );
}

#[test]
fn tombstone_keeps_remaining_ttl() {
    let fx = Fx::new();
    let m = fx.mandate();
    let id = fx.b32(1);
    put(&fx, &m.address, &record(&fx, &id, NOW + 30 * DAY));
    let before = live_until(&fx, &m.address, &id);
    advance(&fx, 10 * DAY);
    fx.env.as_contract(&m.address, || {
        storage::put_tombstone(&fx.env, &id, STATUS_CANCELLED)
    });
    assert_eq!(live_until(&fx, &m.address, &id), before);
    assert!(matches!(m.payout(&id), Some(PayoutEntry::Tombstone(_))));
}

#[test]
fn archived_payout_record_returns_payout_id_exists() {
    let fx = Fx::new();
    let m = fx.mandate();
    let id = fx.b32(1);
    put(&fx, &m.address, &record(&fx, &id, NOW + 3_600));
    let last = live_until(&fx, &m.address, &id);
    // One ledger past the last live one the record is archived; the test
    // host, like a transaction built from simulation, restores it on read.
    fx.env.ledger().set_sequence_number(last + 1);
    fx.env.as_contract(&m.address, || {
        assert_eq!(
            storage::ensure_payout_id_unused(&fx.env, &id),
            Err(MandateError::PayoutIdExists)
        );
        // An ID never written is still unused.
        assert_eq!(
            storage::ensure_payout_id_unused(&fx.env, &fx.b32(2)),
            Ok(())
        );
    });
    assert!(matches!(m.payout(&id), Some(PayoutEntry::Record(_))));
}

#[test]
fn archived_tombstone_returns_payout_id_exists() {
    let fx = Fx::new();
    let m = fx.mandate();
    let id = fx.b32(1);
    put(&fx, &m.address, &record(&fx, &id, NOW + 3_600));
    fx.env.as_contract(&m.address, || {
        storage::put_tombstone(&fx.env, &id, STATUS_CANCELLED)
    });
    let last = live_until(&fx, &m.address, &id);
    // Years later: far past the tombstone's life and the instance's.
    fx.env.ledger().set_sequence_number(last + 10 * 6_307_200);
    fx.env.as_contract(&m.address, || {
        assert_eq!(
            storage::ensure_payout_id_unused(&fx.env, &id),
            Err(MandateError::PayoutIdExists)
        );
    });
}

#[test]
fn instance_ttl_extended_on_every_call() {
    let fx = Fx::new();
    let m = fx.mandate();
    let instance_ttl = |c: &UjiraMandateClient| {
        fx.env
            .as_contract(&c.address, || fx.env.storage().instance().get_ttl())
    };
    assert_eq!(instance_ttl(&m), INSTANCE_TTL_EXTEND_TO);
    // Two days on, every view tops it back up.
    for call in 0..5 {
        advance(&fx, 2 * DAY);
        match call {
            0 => {
                let _ = m.version();
            }
            1 => {
                let _ = m.state();
            }
            2 => {
                let _ = m.config();
            }
            3 => {
                let _ = m.payout(&fx.b32(1));
            }
            _ => {
                let _ = m.daily_used(&fx.b32(1));
            }
        }
        assert_eq!(
            instance_ttl(&m),
            INSTANCE_TTL_EXTEND_TO,
            "after call {call}"
        );
    }
    advance(&fx, 2 * DAY);
    let _ = m.is_allowed(&fx.b32(1), MuxedAddress::from(fx.account(50)));
    assert_eq!(instance_ttl(&m), INSTANCE_TTL_EXTEND_TO);
}

#[test]
fn agent_epoch_stored_on_agent_and_payout() {
    let fx = Fx::new();
    let mut config = fx.config();
    let second = fx.account(21);
    config.agents.push_back(second.clone());
    let id = fx.create(&config).unwrap();
    let m = UjiraMandateClient::new(&fx.env, &id);
    // Agents named at creation are stored with epoch 0 and the counter is 0.
    let roles = fx.env.as_contract(&id, || storage::roles(&fx.env).unwrap());
    assert_eq!(roles.agent_epoch, 0);
    assert_eq!(roles.agent(&fx.agent).map(|a| a.epoch), Some(0));
    assert_eq!(roles.agent(&second).map(|a| a.epoch), Some(0));
    assert_eq!(roles.agent(&fx.approver), None);
    assert_eq!(m.config().agent_epoch, 0);

    // A payout record keeps the epoch its agent held, and reads it back.
    let mut r = record(&fx, &fx.b32(1), NOW + DAY);
    r.agent = second;
    r.agent_epoch = 3;
    put(&fx, &id, &r);
    match m.payout(&fx.b32(1)) {
        Some(PayoutEntry::Record(got)) => assert_eq!(got.agent_epoch, 3),
        other => panic!("expected the record, got {other:?}"),
    }
}

#[test]
fn instance_does_not_grow_with_payouts() {
    // Nothing per payout or per recipient lands in the instance, so its size
    // is bounded by the role set bounds alone.
    let fx = Fx::new();
    let m = fx.mandate();
    let count = || {
        fx.env
            .as_contract(&m.address, || fx.env.storage().instance().all().len())
    };
    let before = count();
    for i in 0..5_u8 {
        put(&fx, &m.address, &record(&fx, &fx.b32(i), NOW + DAY));
    }
    assert_eq!(count(), before);
}
