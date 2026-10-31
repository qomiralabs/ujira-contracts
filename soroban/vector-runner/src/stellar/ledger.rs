// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Classic ledger entries the test host has no call for: accounts and their
//! trustlines. The host checks account signatures against these entries and
//! the Stellar Asset Contract reads trustlines for every account balance, so
//! they are written directly.

use crate::Result;
use soroban_sdk::xdr::{
    AccountEntry, AccountEntryExt, AccountId, Asset, LedgerEntry, LedgerEntryData, LedgerEntryExt,
    LedgerKey, LedgerKeyAccount, LedgerKeyTrustLine, PublicKey, SequenceNumber, Thresholds,
    TrustLineAsset, TrustLineEntry, TrustLineEntryExt, TrustLineFlags, Uint256,
};
use soroban_sdk::Env;
use std::rc::Rc;

pub const AUTHORIZED: u32 = TrustLineFlags::AuthorizedFlag as u32;
pub const CLAWBACK: u32 = TrustLineFlags::TrustlineClawbackEnabledFlag as u32;

fn account_id(pk: [u8; 32]) -> AccountId {
    AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(pk)))
}

fn put(env: &Env, key: LedgerKey, data: LedgerEntryData) -> Result<()> {
    let entry = LedgerEntry {
        data,
        last_modified_ledger_seq: 0,
        ext: LedgerEntryExt::V0,
    };
    env.host()
        .add_ledger_entry(&Rc::new(key), &Rc::new(entry), None)
        .map_err(|e| format!("cannot write a ledger entry: {e:?}"))
}

/// A funded account whose master key alone signs at every threshold, so a
/// single ed25519 signature authorizes its calls.
pub fn create_account(env: &Env, pk: [u8; 32]) -> Result<()> {
    put(
        env,
        LedgerKey::Account(LedgerKeyAccount {
            account_id: account_id(pk),
        }),
        LedgerEntryData::Account(AccountEntry {
            account_id: account_id(pk),
            balance: 10_000_000_000,
            flags: 0,
            home_domain: Default::default(),
            inflation_dest: None,
            num_sub_entries: 2,
            seq_num: SequenceNumber(0),
            thresholds: Thresholds([1, 1, 1, 1]),
            signers: Default::default(),
            ext: AccountEntryExt::V0,
        }),
    )
}

fn line_asset(asset: &Asset) -> TrustLineAsset {
    match asset.clone() {
        Asset::Native => TrustLineAsset::Native,
        Asset::CreditAlphanum4(a) => TrustLineAsset::CreditAlphanum4(a),
        Asset::CreditAlphanum12(a) => TrustLineAsset::CreditAlphanum12(a),
    }
}

pub fn trustline_key(pk: [u8; 32], asset: &Asset) -> LedgerKey {
    LedgerKey::Trustline(LedgerKeyTrustLine {
        account_id: account_id(pk),
        asset: line_asset(asset),
    })
}

/// The account's trustline for `asset`, if it has one.
pub fn trustline(env: &Env, pk: [u8; 32], asset: &Asset) -> Result<Option<TrustLineEntry>> {
    let found = env
        .host()
        .get_ledger_entry(&Rc::new(trustline_key(pk, asset)))
        .map_err(|e| format!("cannot read a trustline: {e:?}"))?;
    Ok(match found {
        Some((entry, _)) => match &entry.data {
            LedgerEntryData::Trustline(t) => Some(t.clone()),
            _ => None,
        },
        None => None,
    })
}

/// Writes the trustline with `flags`, keeping its balance, or opens one at
/// zero.
pub fn set_trustline(env: &Env, pk: [u8; 32], asset: &Asset, flags: u32) -> Result<()> {
    let balance = trustline(env, pk, asset)?.map_or(0, |t| t.balance);
    put(
        env,
        trustline_key(pk, asset),
        LedgerEntryData::Trustline(TrustLineEntry {
            account_id: account_id(pk),
            asset: line_asset(asset),
            balance,
            limit: i64::MAX,
            flags,
            ext: TrustLineEntryExt::V0,
        }),
    )
}
