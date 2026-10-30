// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

// An attribute on a variant, which could hide it from a build.

#![no_std]

use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum MandateError {
    Unauthorized = 10,
    PrincipalChangePending = 26,
    PayoutIdExists = 60,
    #[cfg(test)]
    AgentRemoved = 65,
}
