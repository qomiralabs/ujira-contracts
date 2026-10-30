// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

// Code 65 renamed from AgentRemoved to AgentDropped.

#![no_std]

use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum MandateError {
    Unauthorized = 10,
    PrincipalChangePending = 26,
    PayoutIdExists = 60,
    AgentDropped = 65,
}
