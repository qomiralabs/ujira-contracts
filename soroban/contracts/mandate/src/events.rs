// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The mandate's events. Each has two topics, its snake_case name and a key:
//! the payout ID for payout events and the tenant reference for every other
//! one. The data is a map of the remaining fields, so the key is never
//! repeated there. An `Option` field that is None is left out of the map.

use crate::types::Leg;
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

/// A payout was stored. `task_ref` is carried here and never stored.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayoutRequested {
    #[topic]
    pub payout_id: BytesN<32>,
    pub agent: Address,
    pub recipient_ref: BytesN<32>,
    pub total: i128,
    pub fee: i128,
    pub legs: Vec<Leg>,
    pub task_ref: BytesN<32>,
    pub status: u32,
    pub expires_at: u64,
}

/// One item of a request batch was refused; the batch went on.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayoutRequestRejected {
    #[topic]
    pub payout_id: BytesN<32>,
    pub agent: Address,
    pub code: u32,
}

/// `approvals` is the live count: approvals from addresses that are
/// approvers now, this one included.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayoutApprovalAdded {
    #[topic]
    pub payout_id: BytesN<32>,
    pub approver: Address,
    pub approvals: u32,
    pub required: u32,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayoutApproved {
    #[topic]
    pub payout_id: BytesN<32>,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayoutRejected {
    #[topic]
    pub payout_id: BytesN<32>,
    pub by: Address,
}

/// `reason` is 1 for the principal, 2 for the payout's own agent and 3 for
/// anyone once the payout or the mandate has lapsed.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayoutCancelled {
    #[topic]
    pub payout_id: BytesN<32>,
    pub by: Address,
    pub reason: u32,
}
