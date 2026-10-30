// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

// One of each difference, all reported in one run.

#![no_std]

use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum MandateError {
    PrincipalChangePending = 28,
    PayoutIdTaken = 60,
    AgentRemoved = 65,
    AgentSuspended = 66,
}
