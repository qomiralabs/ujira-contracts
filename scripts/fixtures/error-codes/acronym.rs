// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

// An acronym does not convert back to the table name.

#![no_std]

use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum MandateError {
    Unauthorized = 10,
    PrincipalChangePending = 26,
    PayoutIDExists = 60,
    AgentRemoved = 65,
}
