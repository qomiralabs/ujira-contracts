// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

// A code the check does not read as decimal.

#![no_std]

use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum MandateError {
    Unauthorized = 10,
    PrincipalChangePending = 26,
    PayoutIdExists = 60,
    AgentRemoved = 0x41,
}
