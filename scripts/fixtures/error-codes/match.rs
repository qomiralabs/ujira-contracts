// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

// Matches errors.json beside it. The comments, the quote character and the
// slashes inside the doc string are here to show the check reads past them.

#![no_std]

use soroban_sdk::contracterror;

pub const QUOTE: char = '"';

/* A block comment /* with a nested one */ before the enum. */
#[contracterror]
#[doc = "A // inside a string is not a comment."] #[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum MandateError {
    // Roles.
    Unauthorized = 10,
    /// Lifecycle.
    PrincipalChangePending = 26,
    PayoutIdExists /* the payout ID */ = 60,
    AgentRemoved = 65, // the last one
}
