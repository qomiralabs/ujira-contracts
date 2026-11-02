// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Where the mandate keeps its state, and how long each entry lives.
//!
//! Two kinds of storage, never a third:
//!
//! - Instance storage holds the configuration and the counters. It is one
//!   ledger entry that lives and archives with the contract, and every call
//!   extends its TTL.
//! - Persistent storage holds what grows with use: payout records and their
//!   tombstones, allowlist entries and per-recipient day usage.
//!
//! Temporary storage is never used for rule state, because an expired
//! temporary entry reads as absent: a lapsed tombstone would let a payout ID
//! be used twice. An archived persistent entry still counts as present, since
//! a transaction cannot touch it without restoring it first.
//!
//! Every value is read through a function here that turns a missing entry the
//! constructor always writes into `INTERNAL_INVARIANT`, never a panic.

use crate::types::{DailyUsage, PayoutEntry, PayoutRecord, Tombstone};
use mandate_config::Limits;
use mandate_errors::MandateError;
use soroban_sdk::{
    contracttype, Address, Bytes, BytesN, Env, IntoVal, MuxedAddress, TryFromVal, Val, Vec,
};

/// About one day of ledgers at five seconds each.
pub const DAY_IN_LEDGERS: u32 = 17_280;

/// The instance is kept at least 30 days from expiring. A call tops it up to
/// 31 days once it has fallen a day below, so a busy mandate pays the
/// extension about once a day rather than on every call.
pub const INSTANCE_TTL_EXTEND_TO: u32 = 31 * DAY_IN_LEDGERS;
pub const INSTANCE_TTL_THRESHOLD: u32 = INSTANCE_TTL_EXTEND_TO - DAY_IN_LEDGERS;

/// How long a payout record stays live past its own expiry, so a last moment
/// execute, clock drift and the operator's read after the payout lapses all
/// work without a restore.
pub const RECORD_TTL_MARGIN_SECONDS: u64 = 86_400;

/// The ledger close time used to turn seconds into ledgers. The contract
/// cannot read the real close time; five seconds matches testnet.
pub const STELLAR_LEDGER_SECONDS: u64 = 5;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    // Instance storage.
    Fixed,
    Principal,
    ProposedPrincipal,
    Limits,
    Policy,
    Roles,
    Totals,
    Flags,
    // Persistent storage. A muxed address cannot be a storage key, so an
    // allowlist entry is keyed by its parts: the recipient, the base address
    // and the tag, where no tag and a tag of 0 are different keys.
    Payout(BytesN<32>),
    Allowed(BytesN<32>, Address, Option<u64>),
    Daily(BytesN<32>),
}

/// Set at construction and never written again.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fixed {
    pub factory: Address,
    pub asset: Address,
    pub recipient_mode: u32,
    pub tenant_tag: BytesN<32>,
    pub fee_address: Address,
}

/// The recipient policy: who attests, for which countries, under which
/// epoch, and the fee ceiling, which can only go down.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Policy {
    pub attester: Option<Bytes>,
    pub country_policy_hash: BytesN<32>,
    pub policy_epoch: u64,
    pub max_fee_bps: u32,
}

/// An agent, the `agent_epoch` value it was added under, and what its open
/// payouts hold of `pending` and `committed`. Nothing limits how many payouts
/// can be open, so these sums are what lets a removal release them all in
/// one step instead of a loop over the payouts.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Agent {
    pub account: Address,
    pub epoch: u32,
    pub pending: i128,
    pub committed: i128,
}

/// An approver and the `approver_epoch` value it was added under.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Approver {
    pub account: Address,
    pub epoch: u32,
}

/// Every role set with its gate and its epochs. Each list is kept in the
/// order its members were added: a member is appended on add and taken out
/// on removal without reordering the rest.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Roles {
    pub agents: Vec<Agent>,
    pub approvers: Vec<Approver>,
    pub approvals_required: u32,
    pub guardians: Vec<Address>,
    pub executors: Vec<Address>,
    pub recovery: Vec<Address>,
    pub recovery_threshold: u32,
    pub agent_epoch: u32,
    pub recovery_epoch: u32,
    pub approver_epoch: u32,
}

impl Roles {
    pub fn agent_addresses(&self, env: &Env) -> Vec<Address> {
        let mut out = Vec::new(env);
        for a in self.agents.iter() {
            out.push_back(a.account);
        }
        out
    }

    pub fn approver_addresses(&self, env: &Env) -> Vec<Address> {
        let mut out = Vec::new(env);
        for a in self.approvers.iter() {
            out.push_back(a.account);
        }
        out
    }

    /// The stored entry of `account` when it is an approver.
    pub fn approver(&self, account: &Address) -> Option<Approver> {
        self.approvers.iter().find(|a| a.account == *account)
    }

    pub fn is_approver(&self, account: &Address) -> bool {
        self.approver(account).is_some()
    }

    /// The stored entry of `account` when it is an agent.
    pub fn agent(&self, account: &Address) -> Option<Agent> {
        self.agents.iter().find(|a| a.account == *account)
    }
}

/// The amounts reserved or paid out. `spent + pending + committed` never
/// exceeds the budget.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Totals {
    pub spent: i128,
    pub pending: i128,
    pub committed: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Flags {
    pub paused: bool,
    pub revoked: bool,
}

/// Keeps the instance and the contract code alive. Called by every entry
/// point. The target is clamped to what the network allows, because asking
/// for more fails on older protocols.
pub fn extend_instance(env: &Env) {
    let to = INSTANCE_TTL_EXTEND_TO.min(env.storage().max_ttl());
    let threshold = INSTANCE_TTL_THRESHOLD.min(to);
    env.storage().instance().extend_ttl(threshold, to);
}

fn get_instance<V: TryFromVal<Env, Val>>(env: &Env, key: &DataKey) -> Result<V, MandateError> {
    env.storage()
        .instance()
        .get(key)
        .ok_or(MandateError::InternalInvariant)
}

fn set_instance<V: IntoVal<Env, Val>>(env: &Env, key: &DataKey, value: &V) {
    env.storage().instance().set(key, value);
}

pub fn fixed(env: &Env) -> Result<Fixed, MandateError> {
    get_instance(env, &DataKey::Fixed)
}

pub fn set_fixed(env: &Env, v: &Fixed) {
    set_instance(env, &DataKey::Fixed, v);
}

pub fn principal(env: &Env) -> Result<Address, MandateError> {
    get_instance(env, &DataKey::Principal)
}

pub fn set_principal(env: &Env, v: &Address) {
    set_instance(env, &DataKey::Principal, v);
}

/// None when no principal change is proposed.
pub fn proposed_principal(env: &Env) -> Option<Address> {
    env.storage().instance().get(&DataKey::ProposedPrincipal)
}

pub fn set_proposed_principal(env: &Env, v: Option<&Address>) {
    match v {
        Some(a) => set_instance(env, &DataKey::ProposedPrincipal, a),
        None => env.storage().instance().remove(&DataKey::ProposedPrincipal),
    }
}

pub fn limits(env: &Env) -> Result<Limits, MandateError> {
    get_instance(env, &DataKey::Limits)
}

pub fn set_limits(env: &Env, v: &Limits) {
    set_instance(env, &DataKey::Limits, v);
}

pub fn policy(env: &Env) -> Result<Policy, MandateError> {
    get_instance(env, &DataKey::Policy)
}

pub fn set_policy(env: &Env, v: &Policy) {
    set_instance(env, &DataKey::Policy, v);
}

pub fn roles(env: &Env) -> Result<Roles, MandateError> {
    get_instance(env, &DataKey::Roles)
}

pub fn set_roles(env: &Env, v: &Roles) {
    set_instance(env, &DataKey::Roles, v);
}

pub fn totals(env: &Env) -> Result<Totals, MandateError> {
    get_instance(env, &DataKey::Totals)
}

pub fn set_totals(env: &Env, v: &Totals) {
    set_instance(env, &DataKey::Totals, v);
}

pub fn flags(env: &Env) -> Result<Flags, MandateError> {
    get_instance(env, &DataKey::Flags)
}

pub fn set_flags(env: &Env, v: &Flags) {
    set_instance(env, &DataKey::Flags, v);
}

/// The record or tombstone under `payout_id`, or None if the ID was never
/// used. An archived entry is restored by the transaction that reads it, so
/// it is never None.
pub fn payout(env: &Env, payout_id: &BytesN<32>) -> Option<PayoutEntry> {
    env.storage()
        .persistent()
        .get(&DataKey::Payout(payout_id.clone()))
}

/// Refuses an ID that already has a record or a tombstone.
pub fn ensure_payout_id_unused(env: &Env, payout_id: &BytesN<32>) -> Result<(), MandateError> {
    if env
        .storage()
        .persistent()
        .has(&DataKey::Payout(payout_id.clone()))
    {
        return Err(MandateError::PayoutIdExists);
    }
    Ok(())
}

/// How many ledgers a new record must stay live for: until at least
/// `expires_at` plus the margin, rounded up to whole ledgers, and never more
/// than the network allows. A time already past gives 0, which never
/// shortens anything.
pub fn record_ttl_ledgers(now: u64, expires_at: u64, max_ttl: u32) -> u32 {
    let live_until = expires_at.saturating_add(RECORD_TTL_MARGIN_SECONDS);
    let ledgers = live_until
        .saturating_sub(now)
        .div_ceil(STELLAR_LEDGER_SECONDS);
    u32::try_from(ledgers).unwrap_or(u32::MAX).min(max_ttl)
}

/// Stores a new payout record and extends its TTL to cover its expiry plus
/// the margin. The submitter of the request pays that rent. A new entry
/// starts at the network minimum and an extension never shortens one.
pub fn put_new_record(env: &Env, record: &PayoutRecord) {
    let key = DataKey::Payout(record.payout_id.clone());
    let store = env.storage().persistent();
    store.set(&key, &PayoutEntry::Record(record.clone()));
    let ledgers = record_ttl_ledgers(
        env.ledger().timestamp(),
        record.expires_at,
        env.storage().max_ttl(),
    );
    store.extend_ttl(&key, ledgers, ledgers);
}

/// Writes back a record that is still open, keeping its TTL.
pub fn update_record(env: &Env, record: &PayoutRecord) {
    env.storage().persistent().set(
        &DataKey::Payout(record.payout_id.clone()),
        &PayoutEntry::Record(record.clone()),
    );
}

/// Replaces a record with its tombstone. Nothing extends it: it keeps the
/// record's remaining TTL and is then left to archive, which is safe because
/// an archived tombstone still blocks its ID.
pub fn put_tombstone(env: &Env, payout_id: &BytesN<32>, status: u32) {
    env.storage().persistent().set(
        &DataKey::Payout(payout_id.clone()),
        &PayoutEntry::Tombstone(Tombstone {
            payout_id: payout_id.clone(),
            status,
        }),
    );
}

fn allowed_key(recipient_ref: &BytesN<32>, destination: &MuxedAddress) -> DataKey {
    DataKey::Allowed(
        recipient_ref.clone(),
        destination.address(),
        destination.id(),
    )
}

/// Whether the pair `(recipient_ref, destination)` is on the allowlist. The
/// tag is part of the pair, so another tag at the same address is not.
pub fn is_allowed(env: &Env, recipient_ref: &BytesN<32>, destination: &MuxedAddress) -> bool {
    env.storage()
        .persistent()
        .has(&allowed_key(recipient_ref, destination))
}

pub fn set_allowed(env: &Env, recipient_ref: &BytesN<32>, destination: &MuxedAddress) {
    env.storage()
        .persistent()
        .set(&allowed_key(recipient_ref, destination), &true);
}

pub fn remove_allowed(env: &Env, recipient_ref: &BytesN<32>, destination: &MuxedAddress) {
    env.storage()
        .persistent()
        .remove(&allowed_key(recipient_ref, destination));
}

/// The stored usage of a recipient, whatever day it is for.
pub fn daily(env: &Env, recipient_ref: &BytesN<32>) -> Option<DailyUsage> {
    env.storage()
        .persistent()
        .get(&DataKey::Daily(recipient_ref.clone()))
}

pub fn set_daily(env: &Env, recipient_ref: &BytesN<32>, usage: &DailyUsage) {
    env.storage()
        .persistent()
        .set(&DataKey::Daily(recipient_ref.clone()), usage);
}
