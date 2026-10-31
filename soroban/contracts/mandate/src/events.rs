// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The mandate's events. Each has two topics, its snake_case name and a key:
//! the payout ID for payout events and the tenant reference for every other
//! one. The data is a map of the remaining fields, so the key is never
//! repeated there. An `Option` field that is None is left out of the map.

use crate::types::Leg;
use mandate_config::Limits;
use soroban_sdk::{contractevent, Address, Bytes, BytesN, MuxedAddress, Vec};

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

/// A payout was paid. `day` is the UTC day its total was counted on and
/// `spent` the mandate's spent total after it. The legs are repeated here
/// because the token's own events do not tie a transfer to a payout.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayoutExecuted {
    #[topic]
    pub payout_id: BytesN<32>,
    pub recipient_ref: BytesN<32>,
    pub total: i128,
    pub fee: i128,
    pub legs: Vec<Leg>,
    pub day: u64,
    pub spent: i128,
}

// Lifecycle, administration and recovery. Every one of these is keyed by
// the tenant reference.

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeeCapLowered {
    #[topic]
    pub tenant_ref: BytesN<32>,
    pub max_fee_bps: u32,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Paused {
    #[topic]
    pub tenant_ref: BytesN<32>,
    pub by: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Resumed {
    #[topic]
    pub tenant_ref: BytesN<32>,
    pub by: Address,
}

/// `frozen` is the balance when the revoke landed. It is informational:
/// nothing moved, and the balance can still change through the token.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Revoked {
    #[topic]
    pub tenant_ref: BytesN<32>,
    pub by: Address,
    pub frozen: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Withdrawn {
    #[topic]
    pub tenant_ref: BytesN<32>,
    pub to: Address,
    pub amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Swept {
    #[topic]
    pub tenant_ref: BytesN<32>,
    pub by: Address,
    pub amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenRecovered {
    #[topic]
    pub tenant_ref: BytesN<32>,
    pub token: Address,
    pub amount: i128,
}

/// The six limits as they stand after the change, each its own field.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LimitsUpdated {
    #[topic]
    pub tenant_ref: BytesN<32>,
    pub total_budget: i128,
    pub per_payment_cap: i128,
    pub per_recipient_daily_cap: i128,
    pub approval_threshold: i128,
    pub payout_ttl_seconds: u64,
    pub expires_at: u64,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoleAdded {
    #[topic]
    pub tenant_ref: BytesN<32>,
    pub role: u32,
    pub account: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoleRemoved {
    #[topic]
    pub tenant_ref: BytesN<32>,
    pub role: u32,
    pub account: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovalsRequiredSet {
    #[topic]
    pub tenant_ref: BytesN<32>,
    pub required: u32,
}

/// `recovery_epoch` is the epoch after the change that set the threshold.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryThresholdSet {
    #[topic]
    pub tenant_ref: BytesN<32>,
    pub required: u32,
    pub recovery_epoch: u32,
}

/// `votes` counts this vote, and `recovery_epoch` is the epoch it was cast
/// in, before any increase the action makes. A revoke has no `old` or `new`.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryVoteAdded {
    #[topic]
    pub tenant_ref: BytesN<32>,
    pub kind: u32,
    pub old: Option<Address>,
    pub new: Option<Address>,
    pub votes: u32,
    pub required: u32,
    pub recovery_epoch: u32,
}

/// `by` is the member whose vote completed the action.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryExecuted {
    #[topic]
    pub tenant_ref: BytesN<32>,
    pub kind: u32,
    pub old: Option<Address>,
    pub new: Option<Address>,
    pub by: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecipientAllowed {
    #[topic]
    pub tenant_ref: BytesN<32>,
    pub recipient_ref: BytesN<32>,
    pub destination: MuxedAddress,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecipientDisallowed {
    #[topic]
    pub tenant_ref: BytesN<32>,
    pub recipient_ref: BytesN<32>,
    pub destination: MuxedAddress,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttestationPolicySet {
    #[topic]
    pub tenant_ref: BytesN<32>,
    pub attester: Bytes,
    pub country_policy_hash: BytesN<32>,
    pub policy_epoch: u64,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrincipalProposed {
    #[topic]
    pub tenant_ref: BytesN<32>,
    pub current: Address,
    pub proposed: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrincipalTransferred {
    #[topic]
    pub tenant_ref: BytesN<32>,
    pub previous: Address,
    pub current: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrincipalProposalCancelled {
    #[topic]
    pub tenant_ref: BytesN<32>,
    pub current: Address,
    pub proposed: Address,
}
