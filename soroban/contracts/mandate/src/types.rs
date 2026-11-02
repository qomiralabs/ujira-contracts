// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The values the mandate takes and answers with. Each is a
//! `#[contracttype]`, so a struct crosses the boundary as a map keyed by its
//! field names, so the names are part of the interface.

use mandate_config::Limits;
use soroban_sdk::{contracttype, Address, Bytes, BytesN, MuxedAddress, Vec};

/// Payout statuses. PENDING and APPROVED live in a record; the last three
/// are terminal and live in a tombstone.
pub const STATUS_PENDING: u32 = 1;
pub const STATUS_APPROVED: u32 = 2;
pub const STATUS_EXECUTED: u32 = 3;
pub const STATUS_REJECTED: u32 = 4;
pub const STATUS_CANCELLED: u32 = 5;

/// Role codes, as `add_role` and the role events carry them.
pub const ROLE_AGENT: u32 = 1;
pub const ROLE_APPROVER: u32 = 2;
pub const ROLE_GUARDIAN: u32 = 3;
pub const ROLE_EXECUTOR: u32 = 4;
pub const ROLE_RECOVERY: u32 = 5;

/// One leg of a payout. The destination is a muxed-capable address: an `M`
/// address carries the tag as its muxed id, which the token transfer records.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Leg {
    pub destination: MuxedAddress,
    pub amount: i128,
}

/// A recipient eligibility statement signed by the attester. The signature
/// is variable length on purpose, so a wrong length is refused with a code
/// instead of failing to decode.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attestation {
    pub valid_until: u64,
    pub policy_epoch: u64,
    pub signature: Bytes,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayoutRequest {
    pub payout_id: BytesN<32>,
    pub recipient_ref: BytesN<32>,
    pub total: i128,
    pub fee: i128,
    pub legs: Vec<Leg>,
    pub deadline: u64,
    pub task_ref: BytesN<32>,
    pub attestations: Vec<Attestation>,
}

/// One allowlist entry as the principal passes it.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AllowEntry {
    pub recipient_ref: BytesN<32>,
    pub destination: MuxedAddress,
}

/// One item of a batch result: code 0 is success, and `status` is set only
/// then.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemResult {
    pub payout_id: BytesN<32>,
    pub code: u32,
    pub status: Option<u32>,
}

/// One approval of a payout: who gave it, and the epoch that approver held
/// then, so an approval from before a removal can be told apart from one
/// given after the same address was added back.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Approval {
    pub approver: Address,
    pub epoch: u32,
}

/// A live payout. `agent_epoch` is the value its agent was added under, so a
/// payout from an agent that was removed, or removed and added back, can be
/// told apart from that agent's newer work.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayoutRecord {
    pub payout_id: BytesN<32>,
    pub agent: Address,
    pub agent_epoch: u32,
    pub recipient_ref: BytesN<32>,
    pub total: i128,
    pub fee: i128,
    pub legs: Vec<Leg>,
    pub status: u32,
    pub created_at: u64,
    pub expires_at: u64,
    pub policy_epoch: u64,
    pub approvals: Vec<Approval>,
}

/// What a terminal payout leaves behind, forever, so its ID is never reused.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tombstone {
    pub payout_id: BytesN<32>,
    pub status: u32,
}

/// The one entry kept per payout ID: the record while the payout is open,
/// then the tombstone at the same key, so the tombstone inherits the
/// record's TTL.
///
/// The size lint is allowed because it measures host builds, where every SDK
/// value carries a whole test environment; in the contract each field is a
/// small host handle, and `no_std` has no `Box` to move it behind anyway.
#[allow(clippy::large_enum_variant)]
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PayoutEntry {
    Record(PayoutRecord),
    Tombstone(Tombstone),
}

/// A recipient's executed total on one UTC day.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DailyUsage {
    pub day: u64,
    pub used: i128,
}

/// What `state()` answers. `balance`, `free` and `expired` are derived on
/// every read, never stored.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateView {
    pub spent: i128,
    pub pending: i128,
    pub committed: i128,
    pub balance: i128,
    pub free: i128,
    pub paused: bool,
    pub revoked: bool,
    pub expired: bool,
    pub policy_epoch: u64,
}

/// What `config()` answers: the configuration as it stands now, plus what
/// the mandate keeps beside it. Role lists are in the order they were added.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigView {
    pub principal: Address,
    pub asset: Address,
    pub recipient_mode: u32,
    pub tenant_tag: BytesN<32>,
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
    pub proposed_principal: Option<Address>,
    pub recovery_epoch: u32,
    pub agent_epoch: u32,
    pub approver_epoch: u32,
}
