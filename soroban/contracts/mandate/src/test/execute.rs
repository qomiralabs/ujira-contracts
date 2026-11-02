// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! `execute` and `execute_batch`: the checks that run again at payment
//! time, the order of effects and transfers, the guarded transfers and what
//! a failed one leaves behind, and the batch bounds.

extern crate std;

use super::{account, code, dest, Fx, BOB_WALLET, DAY, FUNDED, NOW};
use crate::events::PayoutExecuted;
use crate::execute::{transfer, MAX_BATCH_TRANSFERS};
use crate::storage::{self, Totals};
use crate::types::{
    DailyUsage, Leg, PayoutRecord, PayoutRequest, ROLE_AGENT, STATUS_APPROVED, STATUS_EXECUTED,
    STATUS_PENDING,
};
use crate::{MandateConfig, MandateError, UjiraMandateClient};
use mandate_config::{MODE_ATTESTED, MODE_BOTH};
use soroban_sdk::{
    testutils::{Address as _, Events as _, IssuerFlags, StellarAssetContract},
    token::{StellarAssetClient, TokenClient},
    xdr::{
        self, AccountEntry, AccountEntryExt, AccountId, LedgerEntry, LedgerEntryData,
        LedgerEntryExt, LedgerKey, LedgerKeyAccount, LedgerKeyTrustLine, ScAddress, ScVal,
        SequenceNumber, Thresholds, TrustLineAsset, TrustLineEntry, TrustLineEntryExt,
    },
    Address, Bytes, BytesN, Event as _, MuxedAddress, Vec,
};
use std::rc::Rc;

const FEE_SEED: u8 = 7;
const TODAY: u64 = NOW / DAY;

/// The base fixture with an asset whose issuer this test can reach, so it
/// can open trustlines and freeze balances.
fn setup() -> (Fx, StellarAssetContract) {
    let mut fx = Fx::new();
    let sac = fx.env.register_stellar_asset_contract_v2(fx.account(99));
    fx.asset = sac.address();
    (fx, sac)
}

fn account_id(a: &Address) -> AccountId {
    match ScAddress::from(a) {
        ScAddress::Account(id) => id,
        other => panic!("not an account address: {other:?}"),
    }
}

fn add_entry(fx: &Fx, key: LedgerKey, data: LedgerEntryData) {
    let entry = LedgerEntry {
        data,
        last_modified_ledger_seq: 0,
        ext: LedgerEntryExt::V0,
    };
    fx.env
        .host()
        .add_ledger_entry(&Rc::new(key), &Rc::new(entry), None)
        .unwrap();
}

/// A classic account with no trustline, like a fresh wallet.
fn open_account(fx: &Fx, seed: u8) -> Address {
    let a = account(&fx.env, seed);
    let id = account_id(&a);
    add_entry(
        fx,
        LedgerKey::Account(LedgerKeyAccount {
            account_id: id.clone(),
        }),
        LedgerEntryData::Account(AccountEntry {
            account_id: id,
            balance: 10_000_000,
            flags: 0,
            home_domain: Default::default(),
            inflation_dest: None,
            num_sub_entries: 0,
            seq_num: SequenceNumber(0),
            thresholds: Thresholds([1; 4]),
            signers: Default::default(),
            ext: AccountEntryExt::V0,
        }),
    );
    a
}

fn trustline(fx: &Fx, sac: &StellarAssetContract, a: &Address, authorized: bool) {
    let id = account_id(a);
    let asset = match sac.asset() {
        xdr::Asset::CreditAlphanum4(x) => TrustLineAsset::CreditAlphanum4(x),
        xdr::Asset::CreditAlphanum12(x) => TrustLineAsset::CreditAlphanum12(x),
        xdr::Asset::Native => TrustLineAsset::Native,
    };
    let flags = if authorized {
        xdr::TrustLineFlags::AuthorizedFlag as u32
    } else {
        0
    };
    add_entry(
        fx,
        LedgerKey::Trustline(LedgerKeyTrustLine {
            account_id: id.clone(),
            asset: asset.clone(),
        }),
        LedgerEntryData::Trustline(TrustLineEntry {
            account_id: id,
            asset,
            balance: 0,
            limit: i64::MAX,
            flags,
            ext: TrustLineEntryExt::V0,
        }),
    );
}

/// A classic account that can receive the asset.
fn wallet(fx: &Fx, sac: &StellarAssetContract, seed: u8) -> Address {
    let a = open_account(fx, seed);
    trustline(fx, sac, &a, true);
    a
}

/// The mandate from `config`, funded, with Bob's wallet allowed, and with
/// Bob's wallet and the fee address able to receive.
fn ready<'a>(
    fx: &'a Fx,
    sac: &StellarAssetContract,
    config: &MandateConfig,
) -> UjiraMandateClient<'a> {
    let m = fx.ready_with(config);
    wallet(fx, sac, BOB_WALLET);
    wallet(fx, sac, FEE_SEED);
    m
}

fn with_fee(fx: &Fx) -> MandateConfig {
    let mut config = fx.config();
    config.max_fee_bps = 100;
    config
}

fn bal(fx: &Fx, who: &Address) -> i128 {
    TokenClient::new(&fx.env, &fx.asset).balance(who)
}

fn bob_addr(fx: &Fx) -> Address {
    account(&fx.env, BOB_WALLET)
}

fn totals(m: &UjiraMandateClient) -> (i128, i128, i128) {
    let st = m.state();
    (st.spent, st.pending, st.committed)
}

fn request(fx: &Fx, m: &UjiraMandateClient, req: &PayoutRequest) {
    assert_eq!(m.request_payout(&fx.agent, req), STATUS_APPROVED);
}

fn approved(fx: &Fx, m: &UjiraMandateClient, id: u8, total: i128) {
    request(fx, m, &fx.req(id, total));
}

/// A request to Bob over the given legs.
fn req_legs(fx: &Fx, id: u8, fee: i128, legs: &[(MuxedAddress, i128)]) -> PayoutRequest {
    let mut req = fx.req(id, 0);
    req.legs = Vec::new(&fx.env);
    let mut total = 0;
    for (d, amount) in legs {
        req.legs.push_back(Leg {
            destination: d.clone(),
            amount: *amount,
        });
        total += amount;
    }
    req.total = total;
    req.fee = fee;
    req
}

fn ids(fx: &Fx, list: &[u8]) -> Vec<BytesN<32>> {
    let mut out = Vec::new(&fx.env);
    for id in list {
        out.push_back(fx.b32(*id));
    }
    out
}

fn i128_of(v: &ScVal) -> i128 {
    match v {
        ScVal::I128(p) => (i128::from(p.hi) << 64) | i128::from(p.lo),
        other => panic!("not an i128: {other:?}"),
    }
}

/// One transfer the asset recorded in the last call.
#[derive(Debug, PartialEq)]
struct Paid {
    to: ScAddress,
    amount: i128,
    muxed_id: Option<u64>,
}

/// Every transfer event the asset emitted in the last call, in order. A
/// transfer to a muxed address carries its id beside the amount.
fn paid(fx: &Fx) -> std::vec::Vec<Paid> {
    let mut out = std::vec::Vec::new();
    for ev in fx.env.events().all().filter_by_contract(&fx.asset).events() {
        let xdr::ContractEventBody::V0(body) = &ev.body;
        match body.topics.first() {
            Some(ScVal::Symbol(s)) if s.0.as_slice() == b"transfer" => {}
            _ => continue,
        }
        let to = match body.topics.get(2) {
            Some(ScVal::Address(a)) => a.clone(),
            other => panic!("transfer without a destination: {other:?}"),
        };
        let (amount, muxed_id) = match &body.data {
            ScVal::Map(Some(map)) => {
                let mut amount = None;
                let mut id = None;
                for e in map.0.iter() {
                    match &e.key {
                        ScVal::Symbol(k) if k.0.as_slice() == b"amount" => {
                            amount = Some(i128_of(&e.val))
                        }
                        ScVal::Symbol(k) if k.0.as_slice() == b"to_muxed_id" => {
                            if let ScVal::U64(v) = e.val {
                                id = Some(v)
                            }
                        }
                        _ => {}
                    }
                }
                (amount.unwrap(), id)
            }
            v => (i128_of(v), None),
        };
        out.push(Paid {
            to,
            amount,
            muxed_id,
        });
    }
    out
}

fn to(fx: &Fx, seed: u8, amount: i128, muxed_id: Option<u64>) -> Paid {
    Paid {
        to: ScAddress::from(&account(&fx.env, seed)),
        amount,
        muxed_id,
    }
}

#[test]
fn execute_pays_each_leg_then_the_fee_and_moves_the_charge_to_spent() {
    let (fx, sac) = setup();
    let m = ready(&fx, &sac, &with_fee(&fx));
    let tagged = dest(&fx.env, BOB_WALLET, Some(9));
    fx.allow(&m.address, &fx.bob(), &tagged);
    let req = req_legs(&fx, 1, 4, &[(fx.bob_wallet(), 300), (tagged, 100)]);
    request(&fx, &m, &req);
    assert_eq!(totals(&m), (0, 0, 404));

    m.execute(&fx.executor, &fx.b32(1));
    assert_eq!(
        fx.events(&m.address),
        [PayoutExecuted {
            payout_id: fx.b32(1),
            recipient_ref: fx.bob(),
            total: 400,
            fee: 4,
            legs: req.legs.clone(),
            day: TODAY,
            spent: 404,
        }
        .to_xdr(&fx.env, &m.address)]
    );
    // Legs in order, then the fee, and nothing else.
    assert_eq!(
        paid(&fx),
        [
            to(&fx, BOB_WALLET, 300, None),
            to(&fx, BOB_WALLET, 100, Some(9)),
            to(&fx, FEE_SEED, 4, None),
        ]
    );
    assert_eq!(totals(&m), (404, 0, 0));
    assert_eq!(m.state().balance, FUNDED - 404);
    assert_eq!(
        m.daily_used(&fx.bob()),
        DailyUsage {
            day: TODAY,
            used: 400
        }
    );
    assert_eq!(fx.tombstone(&m, 1).status, STATUS_EXECUTED);
    assert_eq!(bal(&fx, &bob_addr(&fx)), 400);
    assert_eq!(bal(&fx, &fx.fee_address), 4);
}

#[test]
fn execute_without_a_fee_makes_no_fee_transfer() {
    let (fx, sac) = setup();
    let m = ready(&fx, &sac, &with_fee(&fx));
    approved(&fx, &m, 1, 100);
    m.execute(&fx.agent, &fx.b32(1));
    assert_eq!(paid(&fx), [to(&fx, BOB_WALLET, 100, None)]);
    assert_eq!(bal(&fx, &fx.fee_address), 0);
}

#[test]
fn tagged_leg_emits_to_muxed_id() {
    let (fx, sac) = setup();
    let m = ready(&fx, &sac, &fx.config());
    let tagged = dest(&fx.env, BOB_WALLET, Some(u64::MAX));
    fx.allow(&m.address, &fx.bob(), &tagged);
    let req = req_legs(&fx, 1, 0, &[(tagged.clone(), 100)]);
    request(&fx, &m, &req);
    m.execute(&fx.agent, &fx.b32(1));
    // The tag reaches the token: the transfer names the base account and
    // carries the tag as its muxed id.
    assert_eq!(paid(&fx), [to(&fx, BOB_WALLET, 100, Some(u64::MAX))]);
    assert_eq!(bal(&fx, &bob_addr(&fx)), 100);
}

#[test]
fn execute_by_principal_any_agent_or_executor_only() {
    let (fx, sac) = setup();
    let other_agent = fx.account(21);
    let mut config = fx.config();
    config.agents = fx.list(&[&fx.agent, &other_agent]);
    let m = ready(&fx, &sac, &config);
    for id in 1..=3 {
        approved(&fx, &m, id, 100);
    }
    let stranger = fx.account(40);
    for who in [&fx.approver, &fx.guardian, &fx.recovery, &stranger] {
        assert_eq!(
            code(m.try_execute(who, &fx.b32(1))),
            MandateError::Unauthorized
        );
        assert_eq!(
            code(m.try_execute_batch(who, &ids(&fx, &[1]))),
            MandateError::Unauthorized
        );
    }
    m.execute(&fx.principal, &fx.b32(1));
    m.execute(&fx.executor, &fx.b32(2));
    // An agent may execute a payout another agent requested.
    m.execute(&other_agent, &fx.b32(3));
    assert_eq!(bal(&fx, &bob_addr(&fx)), 300);
}

#[test]
fn execute_caller_active_lookup_status_lapse_and_agent_in_order() {
    let (fx, sac) = setup();
    let m = ready(&fx, &sac, &fx.config());
    approved(&fx, &m, 1, 100);
    assert_eq!(m.request_payout(&fx.agent, &fx.req(2, 500)), STATUS_PENDING);
    let id = fx.b32(1);

    fx.edit_flags(&m.address, |f| {
        f.revoked = true;
        f.paused = true;
    });
    fx.set_time(NOW + 365 * DAY);
    assert_eq!(
        code(m.try_execute(&fx.approver, &id)),
        MandateError::Unauthorized
    );
    assert_eq!(
        code(m.try_execute(&fx.executor, &id)),
        MandateError::MandateRevoked
    );
    fx.edit_flags(&m.address, |f| f.revoked = false);
    assert_eq!(
        code(m.try_execute(&fx.executor, &id)),
        MandateError::MandateExpired
    );
    fx.set_time(NOW);
    assert_eq!(
        code(m.try_execute(&fx.executor, &id)),
        MandateError::MandatePaused
    );
    fx.edit_flags(&m.address, |f| f.paused = false);

    assert_eq!(
        code(m.try_execute(&fx.executor, &fx.b32(9))),
        MandateError::PayoutNotFound
    );
    assert_eq!(
        code(m.try_execute(&fx.executor, &fx.b32(2))),
        MandateError::PayoutWrongStatus
    );
    // Lapsed comes before the agent check.
    m.remove_role(&ROLE_AGENT, &fx.agent);
    fx.set_time(NOW + DAY);
    assert_eq!(
        code(m.try_execute(&fx.executor, &id)),
        MandateError::PayoutLapsed
    );
    fx.set_time(NOW + DAY - 1);
    assert_eq!(
        code(m.try_execute(&fx.executor, &id)),
        MandateError::AgentRemoved
    );
    // Nothing a refused execute does is left behind.
    assert_eq!(totals(&m), (0, 500, 100));
    assert_eq!(fx.record(&m, 1).status, STATUS_APPROVED);
}

#[test]
fn execute_rechecks_caps_binding_daily_headroom_and_balance_in_order() {
    let (fx, sac) = setup();
    let m = ready(&fx, &sac, &with_fee(&fx));
    let req = req_legs(&fx, 1, 4, &[(fx.bob_wallet(), 400)]);
    request(&fx, &m, &req);
    let id = fx.b32(1);
    let addr = m.address.clone();
    let env = &fx.env;

    // The fee cap and the per-payment cap in force now; the fee cap first.
    env.as_contract(&addr, || {
        let mut p = storage::policy(env).unwrap();
        p.max_fee_bps = 0;
        storage::set_policy(env, &p);
        let mut l = storage::limits(env).unwrap();
        l.per_payment_cap = 403;
        storage::set_limits(env, &l);
    });
    assert_eq!(
        code(m.try_execute(&fx.agent, &id)),
        MandateError::FeeExceedsCap
    );
    env.as_contract(&addr, || {
        let mut p = storage::policy(env).unwrap();
        p.max_fee_bps = 100;
        storage::set_policy(env, &p);
    });
    assert_eq!(
        code(m.try_execute(&fx.agent, &id)),
        MandateError::PaymentCapExceeded
    );
    env.as_contract(&addr, || {
        let mut l = storage::limits(env).unwrap();
        l.per_payment_cap = 404;
        storage::set_limits(env, &l);
    });

    // A destination taken off the allowlist stops the payout, before the
    // daily headroom is looked at.
    env.as_contract(&addr, || {
        storage::remove_allowed(env, &fx.bob(), &fx.bob_wallet());
        storage::set_daily(
            env,
            &fx.bob(),
            &DailyUsage {
                day: TODAY,
                used: 101,
            },
        );
    });
    assert_eq!(
        code(m.try_execute(&fx.agent, &id)),
        MandateError::RecipientNotAllowed
    );
    fx.allow(&addr, &fx.bob(), &fx.bob_wallet());
    // 101 used today leaves 399 of 500, one short.
    assert_eq!(
        code(m.try_execute(&fx.agent, &id)),
        MandateError::DailyCapExceeded
    );

    // A full stored day that is not today counts as nothing; then only the
    // balance is short, by one.
    env.as_contract(&addr, || {
        storage::set_daily(
            env,
            &fx.bob(),
            &DailyUsage {
                day: TODAY - 1,
                used: 500,
            },
        )
    });
    let sink = Address::generate(env);
    TokenClient::new(env, &fx.asset).transfer(&addr, &sink, &(FUNDED - 403));
    assert_eq!(
        code(m.try_execute(&fx.agent, &id)),
        MandateError::InsufficientFunds
    );
    fx.mint(&addr, 1);
    m.execute(&fx.agent, &id);
    assert_eq!(m.state().balance, 0);
    assert_eq!(
        m.daily_used(&fx.bob()),
        DailyUsage {
            day: TODAY,
            used: 400
        }
    );
}

/// An approved record put straight into an attested mandate, as a request
/// with a valid attestation would leave it.
fn attested_payout<'a>(
    fx: &'a Fx,
    sac: &StellarAssetContract,
    mode: u32,
) -> UjiraMandateClient<'a> {
    let mut config = fx.config();
    config.recipient_mode = mode;
    config.attester = Some(Bytes::from_array(&fx.env, &[0x42; 32]));
    let m = fx.ready_with(&config);
    wallet(fx, sac, BOB_WALLET);
    let record = PayoutRecord {
        payout_id: fx.b32(1),
        agent: fx.agent.clone(),
        agent_epoch: 0,
        recipient_ref: fx.bob(),
        total: 100,
        fee: 0,
        legs: fx.req(1, 100).legs,
        status: STATUS_APPROVED,
        created_at: NOW,
        expires_at: NOW + 3_600,
        policy_epoch: 0,
        approvals: Vec::new(&fx.env),
    };
    fx.env.as_contract(&m.address, || {
        storage::put_new_record(&fx.env, &record);
        storage::set_totals(
            &fx.env,
            &Totals {
                spent: 0,
                pending: 0,
                committed: 100,
            },
        );
        // The agent's own sum moves with the totals, as a real request does.
        let mut roles = storage::roles(&fx.env).unwrap();
        let mut agent = roles.agents.get(0).unwrap();
        agent.committed = 100;
        roles.agents.set(0, agent);
        storage::set_roles(&fx.env, &roles);
    });
    m
}

fn bump_policy_epoch(fx: &Fx, m: &Address, epoch: u64) {
    fx.env.as_contract(m, || {
        let mut p = storage::policy(&fx.env).unwrap();
        p.policy_epoch = epoch;
        storage::set_policy(&fx.env, &p);
    });
}

#[test]
fn execute_attested_payout_refused_after_a_policy_change() {
    let (fx, sac) = setup();
    let m = attested_payout(&fx, &sac, MODE_ATTESTED);
    bump_policy_epoch(&fx, &m.address, 1);
    assert_eq!(
        code(m.try_execute(&fx.agent, &fx.b32(1))),
        MandateError::PolicyChanged
    );
    // Under the epoch it was attested in it pays, and the allowlist plays no
    // part in this mode.
    bump_policy_epoch(&fx, &m.address, 0);
    fx.env.as_contract(&m.address, || {
        storage::remove_allowed(&fx.env, &fx.bob(), &fx.bob_wallet())
    });
    m.execute(&fx.agent, &fx.b32(1));
    assert_eq!(bal(&fx, &bob_addr(&fx)), 100);
}

#[test]
fn execute_both_mode_checks_the_allowlist_before_the_policy_epoch() {
    let (fx, sac) = setup();
    let m = attested_payout(&fx, &sac, MODE_BOTH);
    bump_policy_epoch(&fx, &m.address, 1);
    fx.env.as_contract(&m.address, || {
        storage::remove_allowed(&fx.env, &fx.bob(), &fx.bob_wallet())
    });
    assert_eq!(
        code(m.try_execute(&fx.agent, &fx.b32(1))),
        MandateError::RecipientNotAllowed
    );
    fx.allow(&m.address, &fx.bob(), &fx.bob_wallet());
    assert_eq!(
        code(m.try_execute(&fx.agent, &fx.b32(1))),
        MandateError::PolicyChanged
    );
}

#[test]
fn transfer_failure_reverts_state_and_events() {
    let (fx, sac) = setup();
    let m = fx.ready_with(&fx.config());
    // Bob's wallet exists but holds no trustline, so it cannot receive.
    let bob = open_account(&fx, BOB_WALLET);
    approved(&fx, &m, 1, 100);

    assert_eq!(
        code(m.try_execute(&fx.agent, &fx.b32(1))),
        MandateError::TransferFailed
    );
    assert_eq!(fx.events(&m.address), []);
    assert_eq!(paid(&fx), []);
    assert_eq!(totals(&m), (0, 0, 100));
    assert_eq!(fx.record(&m, 1).status, STATUS_APPROVED);
    assert_eq!(m.daily_used(&fx.bob()).used, 0);
    assert_eq!(m.state().balance, FUNDED);

    // Once the wallet can receive, the same call pays, exactly once.
    trustline(&fx, &sac, &bob, true);
    m.execute(&fx.agent, &fx.b32(1));
    assert_eq!(
        code(m.try_execute(&fx.agent, &fx.b32(1))),
        MandateError::PayoutWrongStatus
    );
    assert_eq!(bal(&fx, &bob), 100);
    assert_eq!(totals(&m), (100, 0, 0));
}

#[test]
fn two_leg_execute_second_fails_leaves_first_unpaid() {
    let (fx, sac) = setup();
    let m = ready(&fx, &sac, &fx.config());
    // The second leg goes to an account that does not exist.
    let nowhere = dest(&fx.env, 51, None);
    fx.allow(&m.address, &fx.bob(), &nowhere);
    let req = req_legs(&fx, 1, 0, &[(fx.bob_wallet(), 60), (nowhere, 40)]);
    request(&fx, &m, &req);
    assert_eq!(
        code(m.try_execute(&fx.agent, &fx.b32(1))),
        MandateError::TransferFailed
    );
    assert_eq!(fx.events(&m.address), []);
    assert_eq!(bal(&fx, &bob_addr(&fx)), 0);
    assert_eq!(m.state().balance, FUNDED);
    assert_eq!(totals(&m), (0, 0, 100));
    assert_eq!(fx.record(&m, 1).status, STATUS_APPROVED);
}

#[test]
fn failing_fee_transfer_reverts_the_legs() {
    let (fx, sac) = setup();
    let m = fx.ready_with(&with_fee(&fx));
    wallet(&fx, &sac, BOB_WALLET);
    // The fee address cannot receive.
    open_account(&fx, FEE_SEED);
    request(&fx, &m, &req_legs(&fx, 1, 1, &[(fx.bob_wallet(), 100)]));
    assert_eq!(
        code(m.try_execute(&fx.agent, &fx.b32(1))),
        MandateError::TransferFailed
    );
    assert_eq!(bal(&fx, &bob_addr(&fx)), 0);
    assert_eq!(totals(&m), (0, 0, 101));
}

#[test]
fn transfer_failure_missing_trustline_is_transfer_failed() {
    let (fx, _sac) = setup();
    let m = fx.ready_with(&fx.config());
    open_account(&fx, BOB_WALLET);
    approved(&fx, &m, 1, 100);
    // The asset reports its own code 13; the mandate answers 72.
    assert_eq!(
        code(m.try_execute(&fx.agent, &fx.b32(1))),
        MandateError::TransferFailed
    );
    assert_eq!(
        code(m.try_execute_batch(&fx.agent, &ids(&fx, &[1]))),
        MandateError::TransferFailed
    );
}

#[test]
fn transfer_failure_deauthorized_holder_is_transfer_failed() {
    let (fx, sac) = setup();
    let m = fx.ready_with(&fx.config());
    let bob = open_account(&fx, BOB_WALLET);
    // A trustline the issuer has not authorized: the asset's code 11, which
    // is also ROLE_CONFLICT, must never come out of the mandate.
    trustline(&fx, &sac, &bob, false);
    approved(&fx, &m, 1, 100);
    assert_eq!(
        code(m.try_execute(&fx.agent, &fx.b32(1))),
        MandateError::TransferFailed
    );

    // The mandate's own balance frozen by the issuer fails the same way.
    trustline(&fx, &sac, &bob, true);
    sac.issuer().set_flag(IssuerFlags::RevocableFlag);
    StellarAssetClient::new(&fx.env, &fx.asset).set_authorized(&m.address, &false);
    assert_eq!(
        code(m.try_execute(&fx.agent, &fx.b32(1))),
        MandateError::TransferFailed
    );
    StellarAssetClient::new(&fx.env, &fx.asset).set_authorized(&m.address, &true);
    m.execute(&fx.agent, &fx.b32(1));
    assert_eq!(bal(&fx, &bob), 100);
}

#[test]
fn transfer_failure_insufficient_balance_is_transfer_failed() {
    // The balance check before any transfer means execute itself never asks
    // the asset for more than it holds, so the guarded transfer is driven
    // directly here, as the mandate.
    let (fx, sac) = setup();
    let m = fx.ready_with(&fx.config());
    let bob = wallet(&fx, &sac, BOB_WALLET);
    let to = MuxedAddress::from(bob.clone());
    // The asset's code 10, which is also UNAUTHORIZED.
    let got = fx
        .env
        .as_contract(&m.address, || transfer(&fx.env, &fx.asset, &to, FUNDED + 1));
    assert_eq!(got, Err(MandateError::TransferFailed));
    // An error that is not a contract error at all: no contract at the
    // asset address.
    let not_a_token = Address::generate(&fx.env);
    let got = fx
        .env
        .as_contract(&m.address, || transfer(&fx.env, &not_a_token, &to, 1));
    assert_eq!(got, Err(MandateError::TransferFailed));
    let got = fx
        .env
        .as_contract(&m.address, || transfer(&fx.env, &fx.asset, &to, 1));
    assert_eq!(got, Ok(()));
    assert_eq!(bal(&fx, &bob), 1);
}

#[test]
fn payout_above_what_a_trustline_holds_is_transfer_failed() {
    // A classic account holds the asset in a trustline, whose balance is a
    // 64-bit amount, while the mandate's own balance is a contract balance
    // and holds any i128. A payout an account cannot hold is refused by the
    // asset at transfer time and the whole execute reverts.
    let (fx, sac) = setup();
    let big = i128::from(i64::MAX) + 1;
    let mut config = fx.config();
    config.limits = crate::Limits {
        total_budget: i128::MAX,
        per_payment_cap: i128::MAX,
        per_recipient_daily_cap: i128::MAX,
        approval_threshold: i128::MAX,
        payout_ttl_seconds: DAY,
        expires_at: NOW + 365 * DAY,
    };
    let m = ready(&fx, &sac, &config);
    fx.mint(&m.address, big);
    approved(&fx, &m, 1, big);
    assert_eq!(
        code(m.try_execute(&fx.agent, &fx.b32(1))),
        MandateError::TransferFailed
    );
    assert_eq!(totals(&m), (0, 0, big));
    // The same amount to a contract balance goes through.
    let vault = Address::generate(&fx.env);
    let vault_dest = MuxedAddress::from(vault.clone());
    fx.allow(&m.address, &fx.b32(0xc1), &vault_dest);
    let mut req = req_legs(&fx, 2, 0, &[(vault_dest, big)]);
    req.recipient_ref = fx.b32(0xc1);
    m.cancel(&fx.principal, &fx.b32(1));
    request(&fx, &m, &req);
    m.execute(&fx.agent, &fx.b32(2));
    assert_eq!(bal(&fx, &vault), big);
}

#[test]
fn execute_after_remove_role_gives_agent_removed() {
    let (fx, sac) = setup();
    let m = ready(&fx, &sac, &fx.config());
    approved(&fx, &m, 1, 100);
    m.remove_role(&ROLE_AGENT, &fx.agent);
    assert_eq!(
        code(m.try_execute(&fx.executor, &fx.b32(1))),
        MandateError::AgentRemoved
    );
    // The principal can still release it.
    m.cancel(&fx.principal, &fx.b32(1));
    assert_eq!(totals(&m), (0, 0, 0));
    assert_eq!(bal(&fx, &bob_addr(&fx)), 0);
}

#[test]
fn execute_after_readd_gives_agent_removed() {
    let (fx, sac) = setup();
    let m = ready(&fx, &sac, &fx.config());
    approved(&fx, &m, 1, 100);
    // Removed, then added back under a later epoch.
    m.remove_role(&ROLE_AGENT, &fx.agent);
    m.add_role(&ROLE_AGENT, &fx.agent);
    assert_eq!(
        code(m.try_execute(&fx.agent, &fx.b32(1))),
        MandateError::AgentRemoved
    );
    // A payout requested after the re-add is the agent's own and executes.
    approved(&fx, &m, 2, 100);
    m.execute(&fx.agent, &fx.b32(2));
    assert_eq!(bal(&fx, &bob_addr(&fx)), 100);
    assert_eq!(fx.record(&m, 1).status, STATUS_APPROVED);
}

#[test]
fn execute_batch_item_with_removed_agent_fails_batch_with_65() {
    let (fx, sac) = setup();
    let other = fx.account(21);
    let mut config = fx.config();
    config.agents = fx.list(&[&fx.agent, &other]);
    let m = ready(&fx, &sac, &config);
    approved(&fx, &m, 1, 100);
    assert_eq!(m.request_payout(&other, &fx.req(2, 100)), STATUS_APPROVED);
    m.remove_role(&ROLE_AGENT, &other);
    let err = code(m.try_execute_batch(&fx.executor, &ids(&fx, &[1, 2])));
    assert_eq!(err, MandateError::AgentRemoved);
    assert_eq!(err as u32, 65);
    // The first item was paid inside the call and is undone with it.
    assert_eq!(fx.events(&m.address), []);
    assert_eq!(bal(&fx, &bob_addr(&fx)), 0);
    assert_eq!(fx.record(&m, 1).status, STATUS_APPROVED);
    assert_eq!(totals(&m), (0, 0, 200));
    m.execute_batch(&fx.executor, &ids(&fx, &[1]));
    assert_eq!(bal(&fx, &bob_addr(&fx)), 100);
}

#[test]
fn execute_batch_pays_items_in_order_with_one_event_each() {
    let (fx, sac) = setup();
    let m = ready(&fx, &sac, &fx.config());
    approved(&fx, &m, 1, 100);
    approved(&fx, &m, 2, 150);
    m.execute_batch(&fx.agent, &ids(&fx, &[2, 1]));
    assert_eq!(
        fx.events(&m.address),
        [
            PayoutExecuted {
                payout_id: fx.b32(2),
                recipient_ref: fx.bob(),
                total: 150,
                fee: 0,
                legs: fx.req(2, 150).legs,
                day: TODAY,
                spent: 150,
            }
            .to_xdr(&fx.env, &m.address),
            PayoutExecuted {
                payout_id: fx.b32(1),
                recipient_ref: fx.bob(),
                total: 100,
                fee: 0,
                legs: fx.req(1, 100).legs,
                day: TODAY,
                spent: 250,
            }
            .to_xdr(&fx.env, &m.address),
        ]
    );
    assert_eq!(
        paid(&fx),
        [
            to(&fx, BOB_WALLET, 150, None),
            to(&fx, BOB_WALLET, 100, None)
        ]
    );
    assert_eq!(m.daily_used(&fx.bob()).used, 250);
}

#[test]
fn execute_batch_items_accumulate_against_the_daily_cap() {
    let (fx, sac) = setup();
    let m = ready(&fx, &sac, &fx.config());
    // Each fits the cap of 500 against what was executed, so both are
    // requested; together they do not fit.
    approved(&fx, &m, 1, 300);
    approved(&fx, &m, 2, 300);
    assert_eq!(
        code(m.try_execute_batch(&fx.agent, &ids(&fx, &[1, 2]))),
        MandateError::DailyCapExceeded
    );
    assert_eq!(bal(&fx, &bob_addr(&fx)), 0);
    assert_eq!(m.daily_used(&fx.bob()).used, 0);
    m.execute(&fx.agent, &fx.b32(1));
    assert_eq!(
        code(m.try_execute(&fx.agent, &fx.b32(2))),
        MandateError::DailyCapExceeded
    );
    assert_eq!(m.daily_used(&fx.bob()).used, 300);
}

#[test]
fn execute_batch_length_after_caller_and_active_checks() {
    let (fx, sac) = setup();
    let m = ready(&fx, &sac, &fx.config());
    approved(&fx, &m, 1, 10);
    let empty = Vec::new(&fx.env);
    let stranger = fx.account(40);
    assert_eq!(
        code(m.try_execute_batch(&stranger, &empty)),
        MandateError::Unauthorized
    );
    fx.edit_flags(&m.address, |f| f.paused = true);
    assert_eq!(
        code(m.try_execute_batch(&fx.agent, &empty)),
        MandateError::MandatePaused
    );
    fx.edit_flags(&m.address, |f| f.paused = false);
    assert_eq!(
        code(m.try_execute_batch(&fx.agent, &empty)),
        MandateError::BatchInvalid
    );
    let eleven = ids(&fx, &[1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1]);
    assert_eq!(
        code(m.try_execute_batch(&fx.agent, &eleven)),
        MandateError::BatchInvalid
    );
    // The same ID twice: the second sees the first one's tombstone.
    assert_eq!(
        code(m.try_execute_batch(&fx.agent, &ids(&fx, &[1, 1]))),
        MandateError::PayoutWrongStatus
    );
    assert_eq!(fx.record(&m, 1).status, STATUS_APPROVED);
    let ten = ids(&fx, &[1, 1, 1, 1, 1, 1, 1, 1, 1, 1]);
    assert_eq!(
        code(m.try_execute_batch(&fx.agent, &ten)),
        MandateError::PayoutWrongStatus
    );
}

/// A mandate whose daily cap leaves room for a full batch to Bob.
///
/// It also lifts the test host's own invocation limits. With every
/// authorization mocked, the host records an authorization tree for each of
/// the asset's transfers as well and runs out of budget doing so; a real
/// transaction signs only the batch call itself, since the mandate is the
/// direct caller of every transfer. The limits a network enforces are
/// compared by hand afterwards, in `assert_headroom`.
fn roomy(fx: &Fx) -> MandateConfig {
    fx.env.cost_estimate().disable_resource_limits();
    let mut config = with_fee(fx);
    config.limits.per_recipient_daily_cap = 5_000;
    config
}

#[test]
fn execute_batch_18_transfers_ok() {
    let (fx, sac) = setup();
    let m = ready(&fx, &sac, &roomy(&fx));
    // Nine one-leg payouts with a fee: 18 transfers.
    for id in 1..=9 {
        request(&fx, &m, &req_legs(&fx, id, 1, &[(fx.bob_wallet(), 100)]));
    }
    m.execute_batch(&fx.executor, &ids(&fx, &[1, 2, 3, 4, 5, 6, 7, 8, 9]));
    assert_eq!(fx.events(&m.address).len(), 9);
    assert_eq!(paid(&fx).len(), MAX_BATCH_TRANSFERS as usize);
    assert_eq!(bal(&fx, &bob_addr(&fx)), 900);
    assert_eq!(bal(&fx, &fx.fee_address), 9);
    assert_eq!(totals(&m), (909, 0, 0));
}

#[test]
fn execute_batch_19_transfers_with_fee_item_fails() {
    let (fx, sac) = setup();
    let m = ready(&fx, &sac, &roomy(&fx));
    let tagged = dest(&fx.env, BOB_WALLET, Some(1));
    fx.allow(&m.address, &fx.bob(), &tagged);
    let two = [(fx.bob_wallet(), 50), (tagged, 50)];
    for id in 1..=8 {
        request(&fx, &m, &req_legs(&fx, id, 0, &two));
    }
    // Two legs and a fee: 16 + 3 = 19.
    request(&fx, &m, &req_legs(&fx, 9, 1, &two));
    // The same two legs without a fee: 16 + 2 = 18.
    request(&fx, &m, &req_legs(&fx, 10, 0, &two));
    assert_eq!(
        code(m.try_execute_batch(&fx.agent, &ids(&fx, &[1, 2, 3, 4, 5, 6, 7, 8, 9]))),
        MandateError::BatchInvalid
    );
    assert_eq!(fx.events(&m.address), []);
    assert_eq!(bal(&fx, &bob_addr(&fx)), 0);
    assert_eq!(totals(&m), (0, 0, 1_001));
    m.execute_batch(&fx.agent, &ids(&fx, &[1, 2, 3, 4, 5, 6, 7, 8, 10]));
    assert_eq!(bal(&fx, &bob_addr(&fx)), 900);
    assert_eq!(fx.record(&m, 9).status, STATUS_APPROVED);
}

/// Per-transaction network limits, the same on testnet and mainnet.
const INSTRUCTIONS: i64 = 400_000_000;
const MEMORY: i64 = 41_943_040;
const DISK_READ_ENTRIES: u32 = 200;
const DISK_READ_BYTES: u32 = 200_000;
const WRITE_ENTRIES: u32 = 200;
const WRITE_BYTES: u32 = 132_096;
const FOOTPRINT_ENTRIES: u32 = 400;
const EVENTS_BYTES: u32 = 16_384;

/// Every resource of the last call at or under 70 percent of its limit.
fn assert_headroom(fx: &Fx, what: &str) {
    let r = fx.env.cost_estimate().resources();
    let within = |used: i64, limit: i64| used * 10 <= limit * 7;
    let checks = [
        ("instructions", r.instructions, INSTRUCTIONS),
        ("memory", r.mem_bytes, MEMORY),
        (
            "disk_read_entries",
            r.disk_read_entries.into(),
            DISK_READ_ENTRIES.into(),
        ),
        (
            "disk_read_bytes",
            r.disk_read_bytes.into(),
            DISK_READ_BYTES.into(),
        ),
        (
            "write_entries",
            r.write_entries.into(),
            WRITE_ENTRIES.into(),
        ),
        ("write_bytes", r.write_bytes.into(), WRITE_BYTES.into()),
        (
            "footprint",
            i64::from(r.disk_read_entries) + i64::from(r.memory_read_entries),
            FOOTPRINT_ENTRIES.into(),
        ),
        // execute_batch returns nothing, so the events are the whole charge.
        (
            "events",
            r.contract_events_size_bytes.into(),
            EVENTS_BYTES.into(),
        ),
    ];
    for (name, used, limit) in checks {
        std::println!("{what}: {name} {used} of {limit}");
        assert!(within(used, limit), "{what}: {name} {used} of {limit}");
    }
}

/// A fresh wallet per leg, each paid through a tag of full width, so every
/// transfer touches a new trustline and carries the widest event.
fn tagged_wallets(
    fx: &Fx,
    sac: &StellarAssetContract,
    m: &Address,
    first_seed: u8,
    n: u8,
) -> std::vec::Vec<MuxedAddress> {
    (0..n)
        .map(|i| {
            let seed = first_seed + i;
            wallet(fx, sac, seed);
            let d = dest(&fx.env, seed, Some(u64::MAX - u64::from(i)));
            fx.allow(m, &fx.bob(), &d);
            d
        })
        .collect()
}

#[test]
fn execute_batch_at_max_batch_fits_the_resource_limits() {
    let (fx, sac) = setup();
    let m = ready(&fx, &sac, &roomy(&fx));
    // Ten items, 18 tagged legs, no fee: the events worst case.
    let d = tagged_wallets(&fx, &sac, &m.address, 100, 18);
    let mut next = 0;
    for id in 1..=10u8 {
        let n = if id <= 8 { 2 } else { 1 };
        let legs: std::vec::Vec<(MuxedAddress, i128)> =
            (0..n).map(|k| (d[next + k].clone(), 10)).collect();
        next += n;
        request(&fx, &m, &req_legs(&fx, id, 0, &legs));
    }
    m.execute_batch(&fx.executor, &ids(&fx, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]));
    assert_eq!(paid(&fx).len(), 18);
    assert_headroom(&fx, "10 items, 18 tagged legs");
}

#[test]
fn execute_batch_four_full_items_with_fees_fits_the_resource_limits() {
    let (fx, sac) = setup();
    let m = ready(&fx, &sac, &roomy(&fx));
    // Four items of four tagged legs, two with a fee: 18 transfers.
    let d = tagged_wallets(&fx, &sac, &m.address, 130, 16);
    for id in 1..=4u8 {
        let base = usize::from(id - 1) * 4;
        let legs: std::vec::Vec<(MuxedAddress, i128)> =
            (0..4).map(|k| (d[base + k].clone(), 25)).collect();
        let fee = if id <= 2 { 1 } else { 0 };
        request(&fx, &m, &req_legs(&fx, id, fee, &legs));
    }
    m.execute_batch(&fx.executor, &ids(&fx, &[1, 2, 3, 4]));
    assert_eq!(paid(&fx).len(), 18);
    assert_headroom(&fx, "4 items of 4 tagged legs, 2 fees");
}
