// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

#![no_std]

use soroban_sdk::{contract, contractimpl};

#[contract]
pub struct SpikeProbe;

#[contractimpl]
impl SpikeProbe {
    pub fn spike() -> u32 {
        7
    }
}
