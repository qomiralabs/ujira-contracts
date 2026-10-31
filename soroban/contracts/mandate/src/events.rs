// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The mandate's events. Each has two topics, its snake_case name and a key:
//! the payout ID for payout events and the tenant reference for every other
//! one. The data is a map of the remaining fields, so the key is never
//! repeated there. An `Option` field that is None is left out of the map.

use mandate_config::Limits;
use soroban_sdk::{contractevent, Address, Bytes, BytesN, Vec};

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MandateInitialized {
    #[topic]
    pub tenant_ref: BytesN<32>,
    pub principal: Address,
    pub asset: Address,
    pub recipient_mode: u32,
    pub agents: Vec<Address>,
    pub approvers: Vec<Address>,
    pub approvals_required: u32,
    pub guardians: Vec<Address>,
    pub executors: Vec<Address>,
    pub recovery: Vec<Address>,
    pub recovery_threshold: u32,
    pub limits: Limits,
    pub attester: Option<Bytes>,
    pub country_policy_hash: BytesN<32>,
    pub max_fee_bps: u32,
    pub fee_address: Address,
}
