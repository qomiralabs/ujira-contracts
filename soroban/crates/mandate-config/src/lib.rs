// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The mandate's configuration and the one validation of it.
//!
//! The factory validates a configuration before it deploys a mandate, and the
//! mandate's constructor validates it again as a backstop. A constructor error
//! reaches the factory's caller only as a generic host error, so the factory
//! has to find the error itself, and both must pick the same first error for
//! a configuration that breaks several rules. Both call [`validate`] from this
//! crate, so the two can never drift apart.
//!
//! The smaller checks are public too, because later calls on a live mandate
//! (changing limits, roles or the attestation policy) apply the same rules.

#![no_std]

use mandate_errors::MandateError;
use soroban_sdk::{contracttype, xdr::ToXdr, Address, Bytes, BytesN, Env, Vec};

/// What `version()` answers and `mandate_created` carries.
pub const SPEC_VERSION: u32 = 1;

/// Recipient modes.
pub const MODE_ALLOWLIST: u32 = 1;
pub const MODE_ATTESTED: u32 = 2;
pub const MODE_BOTH: u32 = 3;

/// Role set bounds. Every loop over a role set is bounded by these.
pub const MAX_AGENTS: u32 = 8;
pub const MAX_APPROVERS: u32 = 10;
pub const MAX_GUARDIANS: u32 = 4;
pub const MAX_EXECUTORS: u32 = 4;
pub const MAX_RECOVERY: u32 = 10;

/// The highest fee rate a mandate may allow, in basis points (1 percent).
pub const MAX_FEE_BPS: u32 = 100;

/// Bounds on a payout's time to live, in seconds (1 hour to 30 days).
pub const PAYOUT_TTL_MIN: u64 = 3_600;
pub const PAYOUT_TTL_MAX: u64 = 2_592_000;

/// An attester is an ed25519 public key.
pub const ATTESTER_KEY_BYTES: u32 = 32;

/// The six limits, replaced together by `update_limits`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Limits {
    pub total_budget: i128,
    pub per_payment_cap: i128,
    pub per_recipient_daily_cap: i128,
    pub approval_threshold: i128,
    pub payout_ttl_seconds: u64,
    pub expires_at: u64,
}

/// What a principal asks the factory to create. The fee address is not here
/// on purpose: it comes from the factory, so a principal can never pick it.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MandateConfig {
    pub principal: Address,
    pub asset: Address,
    pub recipient_mode: u32,
    pub tenant_ref: BytesN<32>,
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
}

/// Checks a configuration in one fixed order and returns the first failure,
/// so a configuration that breaks several rules always gets the same error.
/// The order: field domains, the attester's presence and form, role
/// separation, the attester key against role accounts, role set bounds, the
/// approval gate, the recovery threshold. The caller and asset checks belong
/// to the factory and run before this. There is no zero address check:
/// Stellar has no zero address.
pub fn validate(
    env: &Env,
    config: &MandateConfig,
    fee_address: &Address,
) -> Result<(), MandateError> {
    let now = env.ledger().timestamp();
    // Field domains and cross-field rules.
    check_domains(config, now)?;
    // The attester is present exactly when the mode needs it.
    check_attester_presence(config.recipient_mode, &config.attester)?;
    // Role separation.
    check_separation(config, fee_address)?;
    // The attester's key is no role account's key.
    if let Some(key) = &config.attester {
        if attester_holds_role(env, key, config, fee_address) {
            return Err(MandateError::RoleConflict);
        }
    }
    // Role set bounds.
    check_bounds(config)?;
    // The approval and recovery gates can be met.
    check_approvals_required(
        config.approvals_required,
        config.approvers.len(),
        &config.limits,
    )?;
    check_recovery_threshold(config.recovery_threshold, config.recovery.len())
}

/// Every value in its domain, with the cross-field rule between the
/// budget and the per-payment cap.
fn check_domains(config: &MandateConfig, now: u64) -> Result<(), MandateError> {
    if !(MODE_ALLOWLIST..=MODE_BOTH).contains(&config.recipient_mode) {
        return Err(MandateError::ConfigInvalid);
    }
    check_limits(&config.limits, now)?;
    if config.max_fee_bps > MAX_FEE_BPS {
        return Err(MandateError::ConfigInvalid);
    }
    // A role list is a set. A repeated member would count twice toward the
    // number of approvers or recovery approvers, which can make a gate
    // impossible to meet, and removing it once would leave it in the role.
    for set in [
        &config.agents,
        &config.approvers,
        &config.guardians,
        &config.executors,
        &config.recovery,
    ] {
        if has_repeat(set) {
            return Err(MandateError::ConfigInvalid);
        }
    }
    Ok(())
}

/// The domains of the six limits and `expires_at > now`. Also the rule a
/// later `update_limits` applies.
pub fn check_limits(limits: &Limits, now: u64) -> Result<(), MandateError> {
    let ok = limits.total_budget >= 1
        && limits.per_payment_cap >= 1
        && limits.per_payment_cap <= limits.total_budget
        && limits.per_recipient_daily_cap >= 1
        && limits.approval_threshold >= 0
        && (PAYOUT_TTL_MIN..=PAYOUT_TTL_MAX).contains(&limits.payout_ttl_seconds)
        && limits.expires_at > now;
    if ok {
        Ok(())
    } else {
        Err(MandateError::ConfigInvalid)
    }
}

/// `ALLOWLIST` takes no attester; `ATTESTED` and `BOTH` need one,
/// and it must be a 32-byte key that is not all zeros. Whether the bytes are
/// a usable curve point is checked off chain.
pub fn check_attester_presence(mode: u32, attester: &Option<Bytes>) -> Result<(), MandateError> {
    match (mode, attester) {
        (MODE_ALLOWLIST, None) => Ok(()),
        (MODE_ALLOWLIST, Some(_)) | (_, None) => Err(MandateError::ConfigInvalid),
        (_, Some(key)) if attester_key_well_formed(key) => Ok(()),
        (_, Some(_)) => Err(MandateError::ConfigInvalid),
    }
}

/// A 32-byte value with at least one non-zero byte.
pub fn attester_key_well_formed(key: &Bytes) -> bool {
    key.len() == ATTESTER_KEY_BYTES && key.iter().any(|b| b != 0)
}

/// Role separation. Every pair not refused here
/// is an overlap the rules allow: the principal may be an approver or a
/// guardian, a guardian may hold any other role, and a recovery approver may
/// be an approver or a guardian.
fn check_separation(config: &MandateConfig, fee_address: &Address) -> Result<(), MandateError> {
    let principal = &config.principal;
    let refused = [
        // The principal is never an agent or an executor.
        config.agents.contains(principal) || config.executors.contains(principal),
        // An agent can never approve its own request.
        overlaps(&config.agents, &config.approvers),
        // Whoever submits is never also the approval gate.
        overlaps(&config.executors, &config.approvers),
        // A spending or submitting key can never vote to rotate or revoke.
        overlaps(&config.recovery, &config.agents) || overlaps(&config.recovery, &config.executors),
        // Agents already execute, so the overlap only blurs who did what.
        overlaps(&config.agents, &config.executors),
        // The recovery set is for when the principal's key is lost.
        config.recovery.contains(principal),
        // The fee address holds no role at all.
        fee_address == principal || in_any_set(fee_address, config),
    ];
    if refused.iter().any(|r| *r) {
        return Err(MandateError::RoleConflict);
    }
    Ok(())
}

/// True when the attester key is the ed25519 key of the principal,
/// the fee address or any role member. Only an account address has such a
/// key; a contract address never matches.
pub fn attester_holds_role(
    env: &Env,
    key: &Bytes,
    config: &MandateConfig,
    fee_address: &Address,
) -> bool {
    if account_key_is(env, &config.principal, key) || account_key_is(env, fee_address, key) {
        return true;
    }
    role_sets(config)
        .iter()
        .any(|set| set.iter().any(|member| account_key_is(env, &member, key)))
}

/// An account address as XDR: the value type (18, an address), the address
/// type (0, an account) and the key type (0, ed25519), then the 32-byte key.
const ACCOUNT_XDR_PREFIX: [u8; 12] = [0, 0, 0, 18, 0, 0, 0, 0, 0, 0, 0, 0];
const ACCOUNT_XDR_BYTES: u32 = 44;

/// Whether `account` is an account address whose ed25519 key is `key`. A
/// contract address encodes to 40 bytes with another address type, so it
/// never matches.
pub fn account_key_is(env: &Env, account: &Address, key: &Bytes) -> bool {
    let xdr = account.clone().to_xdr(env);
    xdr.len() == ACCOUNT_XDR_BYTES
        && xdr.slice(0..12) == Bytes::from_array(env, &ACCOUNT_XDR_PREFIX)
        && xdr.slice(12..ACCOUNT_XDR_BYTES) == *key
}

/// Every role set within its bound.
fn check_bounds(config: &MandateConfig) -> Result<(), MandateError> {
    let within = config.agents.len() <= MAX_AGENTS
        && config.approvers.len() <= MAX_APPROVERS
        && config.guardians.len() <= MAX_GUARDIANS
        && config.executors.len() <= MAX_EXECUTORS
        && config.recovery.len() <= MAX_RECOVERY;
    if within {
        Ok(())
    } else {
        Err(MandateError::RoleSetFull)
    }
}

/// The approval gate, here and for `set_approvals_required`. With no
/// approvers nothing may need an approval, so the requirement is 0 and the
/// threshold sits above the per-payment cap; otherwise the requirement is
/// between 1 and the number of approvers.
pub fn check_approvals_required(
    required: u32,
    approvers: u32,
    limits: &Limits,
) -> Result<(), MandateError> {
    let ok = if approvers == 0 {
        required == 0 && limits.approval_threshold > limits.per_payment_cap
    } else {
        (1..=approvers).contains(&required)
    };
    if ok {
        Ok(())
    } else {
        Err(MandateError::ConfigInvalid)
    }
}

/// The recovery threshold, here and for `set_recovery_threshold`: 0 exactly when
/// the set is empty, otherwise between 1 and the set's size, so a recovery
/// action can never run on a threshold of 0.
pub fn check_recovery_threshold(threshold: u32, members: u32) -> Result<(), MandateError> {
    let ok = if members == 0 {
        threshold == 0
    } else {
        (1..=members).contains(&threshold)
    };
    if ok {
        Ok(())
    } else {
        Err(MandateError::ConfigInvalid)
    }
}

fn role_sets(config: &MandateConfig) -> [&Vec<Address>; 5] {
    [
        &config.agents,
        &config.approvers,
        &config.guardians,
        &config.executors,
        &config.recovery,
    ]
}

fn in_any_set(account: &Address, config: &MandateConfig) -> bool {
    role_sets(config).iter().any(|set| set.contains(account))
}

fn overlaps(a: &Vec<Address>, b: &Vec<Address>) -> bool {
    a.iter().any(|x| b.contains(&x))
}

fn has_repeat(set: &Vec<Address>) -> bool {
    set.iter()
        .enumerate()
        .any(|(i, member)| set.last_index_of(&member) != u32::try_from(i).ok())
}
