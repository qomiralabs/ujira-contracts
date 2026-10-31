// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Recipient attestations: the checks on one leg's attestation and the
//! message the attester signs for it.
//!
//! Every check that can give a code runs before the signature is verified,
//! because a failed ed25519 verification traps in the host and takes the
//! whole call down with it, a whole request batch included. A stale policy
//! epoch in particular gets its own code first, so an honest batch built
//! before a policy change fails per item instead of all at once.

use crate::storage::Policy;
use crate::types::Attestation;
use mandate_errors::MandateError;
use soroban_sdk::{xdr::ToXdr, Bytes, BytesN, Env, MuxedAddress};

/// The longest an attestation may stay valid for, in seconds.
pub const MAX_ATTESTATION_VALIDITY: u64 = 86_400;

/// An ed25519 signature.
pub const ATTESTATION_SIGNATURE_BYTES: u32 = 64;

/// Versions the signed message. Another scheme takes another prefix and
/// never this one.
pub const MESSAGE_PREFIX: &[u8; 15] = b"UJIRA/ATTEST/V2";

/// The attester's key, as the constructor and the policy call stored it.
/// An attested mode always has one, so a missing or misshapen key means the
/// stored policy is broken, not that the request is.
pub fn attester_key(policy: &Policy) -> Result<BytesN<32>, MandateError> {
    match &policy.attester {
        Some(key) => BytesN::try_from(key.clone()).map_err(|_| MandateError::InternalInvariant),
        None => Err(MandateError::InternalInvariant),
    }
}

/// Checks one leg's attestation and returns its `valid_until`. The order
/// matters: expiry, then the window, then the signature length, then the
/// policy epoch, and only then the signature itself, which traps on
/// failure rather than returning.
pub fn check_leg(
    env: &Env,
    attester: &BytesN<32>,
    policy: &Policy,
    recipient_ref: &BytesN<32>,
    destination: &MuxedAddress,
    att: &Attestation,
    now: u64,
) -> Result<u64, MandateError> {
    if now >= att.valid_until {
        return Err(MandateError::AttestationExpired);
    }
    // Only after the expiry check, so valid_until > now and this cannot
    // underflow, and no now + MAX sum that could overflow is ever formed.
    let window = att
        .valid_until
        .checked_sub(now)
        .ok_or(MandateError::InternalInvariant)?;
    if window > MAX_ATTESTATION_VALIDITY {
        return Err(MandateError::AttestationInvalid);
    }
    // The signature arrives as variable-length bytes so a wrong length can
    // be refused with a code here; a fixed-size argument would fail to
    // decode before any of this ran.
    if att.signature.len() != ATTESTATION_SIGNATURE_BYTES {
        return Err(MandateError::AttestationInvalid);
    }
    let signature = BytesN::<64>::try_from(att.signature.clone())
        .map_err(|_| MandateError::InternalInvariant)?;
    if att.policy_epoch != policy.policy_epoch {
        return Err(MandateError::PolicyChanged);
    }
    let msg = message(
        env,
        recipient_ref,
        &policy.country_policy_hash,
        att.policy_epoch,
        destination,
        att.valid_until,
    )?;
    // Traps on any failure. The contract cannot catch it, and must not try:
    // a wrongly signed attestation is never something to skip past.
    env.crypto().ed25519_verify(attester, &msg, &signature);
    Ok(att.valid_until)
}

/// The 47 bytes the attester signs: the prefix and the SHA-256 of the
/// attested fields. The amount and the fee are deliberately not in it.
pub fn message(
    env: &Env,
    recipient_ref: &BytesN<32>,
    country_policy_hash: &BytesN<32>,
    policy_epoch: u64,
    destination: &MuxedAddress,
    valid_until: u64,
) -> Result<Bytes, MandateError> {
    let fields = fields(
        env,
        recipient_ref,
        country_policy_hash,
        policy_epoch,
        destination,
        valid_until,
    )?;
    let mut msg = Bytes::from_array(env, MESSAGE_PREFIX);
    msg.append(&env.crypto().sha256(&fields).to_bytes().into());
    Ok(msg)
}

/// Every field has a fixed width or a length prefix, so no two different
/// field sequences can encode to the same bytes. The network ID binds the
/// chain and the mandate's own address binds this mandate, so an
/// attestation can never be replayed elsewhere. The tag is bound with its
/// presence: no tag and a tag of 0 sign differently.
pub fn fields(
    env: &Env,
    recipient_ref: &BytesN<32>,
    country_policy_hash: &BytesN<32>,
    policy_epoch: u64,
    destination: &MuxedAddress,
    valid_until: u64,
) -> Result<Bytes, MandateError> {
    let mandate = env.current_contract_address().to_xdr(env);
    let dest = destination.address().to_xdr(env);
    let (has_tag, tag) = match destination.id() {
        Some(id) => (1u8, id),
        None => (0u8, 0u64),
    };
    let mut b: Bytes = env.ledger().network_id().into();
    b.push_back(len_byte(&mandate)?);
    b.append(&mandate);
    b.append(&recipient_ref.clone().into());
    b.append(&country_policy_hash.clone().into());
    b.extend_from_array(&policy_epoch.to_be_bytes());
    b.extend_from_array(&valid_until.to_be_bytes());
    b.push_back(len_byte(&dest)?);
    b.append(&dest);
    b.push_back(has_tag);
    b.extend_from_array(&tag.to_be_bytes());
    Ok(b)
}

/// An address encodes to 40 or 44 bytes, so this never fails today. It is a
/// checked conversion, not a cast, in case a longer address type appears.
fn len_byte(b: &Bytes) -> Result<u8, MandateError> {
    u8::try_from(b.len()).map_err(|_| MandateError::InternalInvariant)
}
