// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

// AGENT_REMOVED (65) has no variant.

#![no_std]

use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum MandateError {
    Unauthorized = 10,
    PrincipalChangePending = 26,
    PayoutIdExists = 60,
}
