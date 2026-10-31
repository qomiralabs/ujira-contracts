// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Values computed on every read and never stored: the balance, whether the
//! mandate is expired or active, the free funds and today's day number.
//! Storing any of them would let it drift from the token or the clock; a
//! plain transfer into the mandate, for one, changes the balance without
//! calling the mandate at all.

use crate::storage::Totals;
use mandate_errors::MandateError;
use soroban_sdk::{token::TokenClient, Address, Env};

pub const SECONDS_PER_DAY: u64 = 86_400;

/// The mandate's balance of the settlement asset, read from the token. A
/// balance is an `i128`, so it can never exceed the largest amount. A read
/// that fails is a host abort, not a mandate error: the call did not land.
pub fn balance(env: &Env, asset: &Address) -> i128 {
    TokenClient::new(env, asset).balance(&env.current_contract_address())
}

/// At or after `expires_at` the mandate is expired.
pub fn is_expired(now: u64, expires_at: u64) -> bool {
    now >= expires_at
}

pub fn is_active(revoked: bool, now: u64, expires_at: u64) -> bool {
    !revoked && !is_expired(now, expires_at)
}

/// While active, what is left after the reservations, never below zero;
/// once revoked or expired, the whole balance, since nothing reserved can
/// execute any more.
pub fn free(balance: i128, totals: &Totals, active: bool) -> Result<i128, MandateError> {
    if !active {
        return Ok(balance);
    }
    // Both are within the budget, so neither the sum nor the difference
    // can overflow; checked all the same so a broken invariant is a code.
    let reserved = totals
        .pending
        .checked_add(totals.committed)
        .ok_or(MandateError::InternalInvariant)?;
    if balance <= reserved {
        return Ok(0);
    }
    balance
        .checked_sub(reserved)
        .ok_or(MandateError::InternalInvariant)
}

/// The UTC day number of a ledger time.
pub fn day_of(now: u64) -> u64 {
    now / SECONDS_PER_DAY
}
