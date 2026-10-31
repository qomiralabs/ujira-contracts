// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Pause and resume, revoke, withdraw, sweep and token recovery: who may
//! call them, their guards, and that funds only ever go to the principal.

extern crate std;

use super::{code, Fx, DAY, FUNDED, NOW};
use crate::events::{Paused, Resumed, Revoked, Swept, TokenRecovered, Withdrawn};
use crate::payout::SWEEP_GRACE_SECONDS;
use crate::{MandateConfig, MandateError, UjiraMandateClient};
use soroban_sdk::{
    testutils::{Address as _, Events as _},
    token::{StellarAssetClient, TokenClient},
    xdr::{
        AccountEntry, AccountEntryExt, AccountId, Asset, LedgerEntry, LedgerEntryData,
        LedgerEntryExt, LedgerKey, LedgerKeyAccount, LedgerKeyTrustLine, PublicKey, SequenceNumber,
        Thresholds, TrustLineAsset, TrustLineEntry, TrustLineEntryExt, TrustLineFlags, Uint256,
    },
    Address, Event as _,
};
use std::rc::Rc;

/// A mandate whose principal is a contract, which holds the asset without
/// a trustline, so transfers to it simply land.
fn with_contract_principal(fx: &Fx) -> (UjiraMandateClient<'_>, Address) {
    let principal = Address::generate(&fx.env);
    let mut config = fx.config();
    config.principal = principal.clone();
    (fx.ready_with(&config), principal)
}

fn balance(fx: &Fx, token: &Address, who: &Address) -> i128 {
    TokenClient::new(&fx.env, token).balance(who)
}

#[test]
fn pause_by_each_allowed_role_and_resume_by_the_principal() {
    let fx = Fx::new();
    let m = fx.ready();
    let tenant = fx.b32(0x7e);
    for who in [&fx.principal, &fx.approver, &fx.guardian, &fx.recovery] {
        m.pause(who);
        assert_eq!(
            fx.events(&m.address),
            [Paused {
                tenant_ref: tenant.clone(),
                by: who.clone(),
            }
            .to_xdr(&fx.env, &m.address)]
        );
        assert!(m.state().paused);
        assert_eq!(code(m.try_pause(who)), MandateError::AlreadyPaused);
        m.resume();
        assert_eq!(
            fx.events(&m.address),
            [Resumed {
                tenant_ref: tenant.clone(),
                by: fx.principal.clone(),
            }
            .to_xdr(&fx.env, &m.address)]
        );
        assert!(!m.state().paused);
    }
    assert_eq!(code(m.try_resume()), MandateError::NotPaused);
    let stranger = fx.account(40);
    for who in [&fx.agent, &fx.executor, &fx.fee_address, &stranger] {
        assert_eq!(code(m.try_pause(who)), MandateError::Unauthorized);
    }
    // Pausing never needs the mandate to be active.
    fx.set_time(NOW + 400 * DAY);
    m.pause(&fx.guardian);
}

fn account_id(seed: u8) -> AccountId {
    AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([seed; 32])))
}

fn put(fx: &Fx, key: LedgerKey, data: LedgerEntryData) {
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

/// Opens the account `seed` with a trustline for `asset` carrying `flags`,
/// the way a classic account receives a Stellar asset.
fn trustline(fx: &Fx, seed: u8, asset: &Asset, flags: u32) {
    put(
        fx,
        LedgerKey::Account(LedgerKeyAccount {
            account_id: account_id(seed),
        }),
        LedgerEntryData::Account(AccountEntry {
            account_id: account_id(seed),
            balance: 10_000_000_000,
            flags: 0,
            home_domain: Default::default(),
            inflation_dest: None,
            num_sub_entries: 1,
            seq_num: SequenceNumber(0),
            thresholds: Thresholds([1, 1, 1, 1]),
            signers: Default::default(),
            ext: AccountEntryExt::V0,
        }),
    );
    let line = match asset.clone() {
        Asset::CreditAlphanum4(a) => TrustLineAsset::CreditAlphanum4(a),
        other => panic!("unexpected asset {other:?}"),
    };
    put(
        fx,
        LedgerKey::Trustline(LedgerKeyTrustLine {
            account_id: account_id(seed),
            asset: line.clone(),
        }),
        LedgerEntryData::Trustline(TrustLineEntry {
            account_id: account_id(seed),
            asset: line,
            balance: 0,
            limit: i64::MAX,
            flags,
            ext: TrustLineEntryExt::V0,
        }),
    );
}

#[test]
fn revoke_makes_no_token_call_and_returns_frozen() {
    let fx = Fx::new();
    // The principal is a classic account, account 1, with no trustline yet.
    let sac = fx.env.register_stellar_asset_contract_v2(fx.account(98));
    let asset = sac.address();
    let mut config: MandateConfig = fx.config();
    config.asset = asset.clone();
    let id = fx.create(&config).unwrap();
    StellarAssetClient::new(&fx.env, &asset).mint(&id, &FUNDED);
    fx.allow(&id, &fx.bob(), &fx.bob_wallet());
    let m = UjiraMandateClient::new(&fx.env, &id);
    m.request_payout(&fx.agent, &fx.req(1, 500));
    m.request_payout(&fx.agent, &fx.req(2, 100));

    assert_eq!(m.revoke(), FUNDED);
    assert_eq!(
        fx.events(&id),
        [Revoked {
            tenant_ref: fx.b32(0x7e),
            by: fx.principal.clone(),
            frozen: FUNDED,
        }
        .to_xdr(&fx.env, &id)]
    );
    // No transfer, so the asset emitted nothing and nothing moved.
    assert!(fx
        .env
        .events()
        .all()
        .filter_by_contract(&asset)
        .events()
        .is_empty());
    let st = m.state();
    assert!(st.revoked);
    assert_eq!((st.pending, st.committed), (0, 0));
    assert_eq!((st.balance, st.free), (FUNDED, FUNDED));
    assert_eq!(balance(&fx, &asset, &id), FUNDED);
    assert_eq!(code(m.try_revoke()), MandateError::MandateRevoked);

    // Without a trustline the principal cannot receive yet: withdraw fails
    // with nothing changed and can be retried.
    assert_eq!(code(m.try_withdraw(&100)), MandateError::TransferFailed);
    // A deauthorized trustline is the same.
    trustline(&fx, 1, &sac.asset(), 0);
    assert_eq!(code(m.try_withdraw(&100)), MandateError::TransferFailed);
    assert_eq!(balance(&fx, &asset, &id), FUNDED);
    // Once it can receive, the withdraw lands.
    trustline(&fx, 1, &sac.asset(), TrustLineFlags::AuthorizedFlag as u32);
    m.withdraw(&FUNDED);
    assert_eq!(balance(&fx, &asset, &fx.principal), FUNDED);
    assert_eq!(balance(&fx, &asset, &id), 0);
}

#[test]
fn revoke_while_paused_and_expired_and_nothing_role_gated_after() {
    let fx = Fx::new();
    let m = fx.ready();
    m.request_payout(&fx.agent, &fx.req(1, 100));
    m.pause(&fx.guardian);
    fx.set_time(NOW + 400 * DAY);
    assert_eq!(m.revoke(), FUNDED);
    assert_eq!(
        code(m.try_pause(&fx.guardian)),
        MandateError::MandateRevoked
    );
    assert_eq!(code(m.try_resume()), MandateError::MandateRevoked);
    assert_eq!(code(m.try_revoke()), MandateError::MandateRevoked);
    assert_eq!(
        code(m.try_add_role(&3, &fx.account(41))),
        MandateError::MandateRevoked
    );
    assert_eq!(code(m.try_lower_fee_cap(&0)), MandateError::MandateRevoked);
    // Role storage stays readable.
    assert_eq!(m.config().agents.len(), 1);
}

#[test]
fn withdraw_is_capped_at_free_while_active() {
    let fx = Fx::new();
    let (m, principal) = with_contract_principal(&fx);
    m.request_payout(&fx.agent, &fx.req(1, 500));
    m.request_payout(&fx.agent, &fx.req(2, 100));
    let free = FUNDED - 600;
    assert_eq!(m.state().free, free);
    assert_eq!(code(m.try_withdraw(&0)), MandateError::AmountInvalid);
    assert_eq!(code(m.try_withdraw(&-1)), MandateError::AmountInvalid);
    assert_eq!(
        code(m.try_withdraw(&(free + 1))),
        MandateError::InsufficientFunds
    );
    m.withdraw(&free);
    assert_eq!(
        fx.events(&m.address),
        [Withdrawn {
            tenant_ref: fx.b32(0x7e),
            to: principal.clone(),
            amount: free,
        }
        .to_xdr(&fx.env, &m.address)]
    );
    assert_eq!(balance(&fx, &fx.asset, &principal), free);
    let st = m.state();
    assert_eq!(
        (st.balance, st.free, st.pending, st.committed),
        (600, 0, 500, 100)
    );
    // Allowed while paused too.
    m.pause(&fx.guardian);
    assert_eq!(code(m.try_withdraw(&1)), MandateError::InsufficientFunds);
    // Once expired, the whole balance is free.
    fx.set_time(NOW + 365 * DAY);
    m.withdraw(&600);
    assert_eq!(balance(&fx, &fx.asset, &principal), FUNDED);
}

#[test]
fn sweep_guards_in_order_and_pays_only_the_principal() {
    let fx = Fx::new();
    let (m, principal) = with_contract_principal(&fx);
    let stranger = Address::generate(&fx.env);
    assert_eq!(code(m.try_sweep(&stranger)), MandateError::MandateActive);
    assert_eq!(code(m.try_sweep(&principal)), MandateError::MandateActive);

    let expiry = NOW + 365 * DAY;
    fx.set_time(expiry);
    assert_eq!(code(m.try_sweep(&stranger)), MandateError::SweepGracePeriod);
    fx.set_time(expiry + SWEEP_GRACE_SECONDS - 1);
    assert_eq!(code(m.try_sweep(&stranger)), MandateError::SweepGracePeriod);
    fx.set_time(expiry + SWEEP_GRACE_SECONDS);
    // A pending principal change holds strangers but never the principal.
    m.propose_principal(&fx.account(42));
    assert_eq!(
        code(m.try_sweep(&stranger)),
        MandateError::PrincipalChangePending
    );
    m.cancel_principal_proposal();
    assert_eq!(m.sweep(&stranger), FUNDED);
    assert_eq!(
        fx.events(&m.address),
        [Swept {
            tenant_ref: fx.b32(0x7e),
            by: stranger.clone(),
            amount: FUNDED,
        }
        .to_xdr(&fx.env, &m.address)]
    );
    assert_eq!(balance(&fx, &fx.asset, &principal), FUNDED);
    assert_eq!(balance(&fx, &fx.asset, &stranger), 0);
    assert_eq!(code(m.try_sweep(&stranger)), MandateError::NothingToSweep);
}

#[test]
fn sweep_by_the_principal_in_the_grace_and_by_anyone_after_revoke() {
    let fx = Fx::new();
    let (m, principal) = with_contract_principal(&fx);
    fx.set_time(NOW + 365 * DAY);
    m.propose_principal(&fx.account(42));
    assert_eq!(m.sweep(&principal), FUNDED);

    let fx = Fx::new();
    let (m, principal) = with_contract_principal(&fx);
    m.revoke();
    let stranger = Address::generate(&fx.env);
    // After a revoke there is no grace.
    assert_eq!(m.sweep(&stranger), FUNDED);
    assert_eq!(balance(&fx, &fx.asset, &principal), FUNDED);
}

#[test]
fn recover_token_returns_other_tokens_only() {
    let fx = Fx::new();
    let (m, principal) = with_contract_principal(&fx);
    let other = fx
        .env
        .register_stellar_asset_contract_v2(fx.account(97))
        .address();
    StellarAssetClient::new(&fx.env, &other).mint(&m.address, &50);

    assert_eq!(
        code(m.try_recover_token(&fx.asset, &1)),
        MandateError::WrongAsset
    );
    assert_eq!(
        code(m.try_recover_token(&other, &0)),
        MandateError::AmountInvalid
    );
    // More than the mandate holds: the token refuses, and that is
    // TRANSFER_FAILED whatever code the token used.
    assert_eq!(
        code(m.try_recover_token(&other, &51)),
        MandateError::TransferFailed
    );
    m.recover_token(&other, &50);
    assert_eq!(
        fx.events(&m.address),
        [TokenRecovered {
            tenant_ref: fx.b32(0x7e),
            token: other.clone(),
            amount: 50,
        }
        .to_xdr(&fx.env, &m.address)]
    );
    assert_eq!(balance(&fx, &other, &principal), 50);
    // The settlement balance did not move, and recovery works when revoked.
    assert_eq!(m.state().balance, FUNDED);
    m.revoke();
    StellarAssetClient::new(&fx.env, &other).mint(&m.address, &5);
    m.recover_token(&other, &5);
}
