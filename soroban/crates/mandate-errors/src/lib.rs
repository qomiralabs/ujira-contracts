// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Error codes for the mandate and the factory.
//!
//! Callers, indexers and the off-chain decoder only ever see the number, so
//! every contract imports this one enum instead of declaring its own, and a
//! number can never mean two things. The codes come from spec/errors.json,
//! which is append only and never reuses a code. scripts/check-error-codes.py
//! fails CI when this enum and that table differ in any name or number.

#![no_std]

use soroban_sdk::contracterror;

/// Why a mandate or factory call was refused.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum MandateError {
    // Roles.
    Unauthorized = 10,
    RoleConflict = 11,
    RoleSetFull = 12,

    // Lifecycle: revoke, expiry, pause and sweep.
    MandateRevoked = 20,
    MandateExpired = 21,
    MandatePaused = 22,
    AlreadyPaused = 23,
    NotPaused = 24,
    MandateActive = 25,
    PrincipalChangePending = 26,
    SweepGracePeriod = 27,

    // Arguments and configuration.
    AmountInvalid = 30,
    LegsInvalid = 31,
    LegsSumMismatch = 32,
    BatchInvalid = 33,
    ConfigInvalid = 34,
    RequestDeadlinePassed = 35,

    // Limits and balances.
    BudgetExceeded = 40,
    PaymentCapExceeded = 41,
    DailyCapExceeded = 42,
    InsufficientFunds = 43,
    BudgetBelowReserved = 44,
    NothingToSweep = 45,
    FeeExceedsCap = 46,

    // Recipients and attestations.
    RecipientNotAllowed = 50,
    AttestationInvalid = 51,
    AttestationExpired = 52,
    PolicyChanged = 53,
    WrongRecipientMode = 54,

    // Payouts.
    PayoutIdExists = 60,
    PayoutNotFound = 61,
    PayoutWrongStatus = 62,
    PayoutLapsed = 63,
    AlreadyApproved = 64,
    AgentRemoved = 65,

    // Assets and transfers.
    AssetNotSupported = 70,
    WrongAsset = 71,
    TransferFailed = 72,

    // Checked arithmetic that failed where the rules say it cannot. Returned
    // as an error rather than a trap, so the cause is still readable.
    InternalInvariant = 90,
}

#[cfg(test)]
mod test;
