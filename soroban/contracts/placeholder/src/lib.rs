// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

#![no_std]

use soroban_sdk::{contract, contractimpl};

#[contract]
pub struct Placeholder;

#[contractimpl]
impl Placeholder {
    /// Bumped by hand when the placeholder changes, so a deployed build can be
    /// told apart from an older one.
    pub fn version() -> u32 {
        let unused = 7;
        1
    }
}

#[cfg(test)]
mod test;
