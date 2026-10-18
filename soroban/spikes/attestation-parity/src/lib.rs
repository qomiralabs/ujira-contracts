// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Spike only, never deployed outside testnet. Verifies per-leg recipient
//! attestations the way the mandate will, so every accept or reject decision
//! can be compared with the reference signer's fixture.

#![no_std]

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, panic_with_error,
    xdr::ToXdr, Bytes, BytesN, Env, MuxedAddress, Vec,
};

/// Codes match the mandate's shared error table.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    ConfigInvalid = 34,
    AttestationInvalid = 51,
    AttestationExpired = 52,
    PolicyChanged = 53,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Leg {
    pub destination: MuxedAddress,
    pub amount: i128,
}

/// The signature is `Bytes`, not `BytesN<64>`, so a short or long signature
/// reaches the contract and gets a contract error code, instead of failing
/// argument decoding before any contract code runs.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attestation {
    pub valid_until: u64,
    pub policy_epoch: u64,
    pub signature: Bytes,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttestationPolicySet {
    pub attester: BytesN<32>,
    pub country_policy_hash: BytesN<32>,
    pub policy_epoch: u64,
}

#[contracttype]
enum Key {
    Attester,
    PolicyHash,
    PolicyEpoch,
}

pub const PREFIX: &[u8; 15] = b"UJIRA/ATTEST/V2";
pub const MESSAGE_BYTES: u32 = 47;
pub const SIGNATURE_BYTES: u32 = 64;
const MAX_ATTESTATION_VALIDITY: u64 = 86_400;

#[contract]
pub struct AttestationParity;

#[contractimpl]
impl AttestationParity {
    /// The mandate starts at policy epoch 0 and bumps it on every policy
    /// change. The spike takes the epoch directly so the fixture can put a
    /// mandate at any epoch without replaying policy changes.
    pub fn __constructor(
        env: Env,
        attester: Bytes,
        country_policy_hash: BytesN<32>,
        policy_epoch: u64,
    ) {
        // Variable length on purpose, so a key of the wrong size is a
        // contract error rather than an argument decoding failure.
        let attester: BytesN<32> = match attester.try_into() {
            Ok(key) => key,
            Err(_) => panic_with_error!(&env, Error::ConfigInvalid),
        };
        if attester.to_array() == [0u8; 32] {
            panic_with_error!(&env, Error::ConfigInvalid);
        }
        let store = env.storage().instance();
        store.set(&Key::Attester, &attester);
        store.set(&Key::PolicyHash, &country_policy_hash);
        store.set(&Key::PolicyEpoch, &policy_epoch);
        AttestationPolicySet {
            attester,
            country_policy_hash,
            policy_epoch,
        }
        .publish(&env);
    }

    /// The attested fields for one leg, from this contract's own network and
    /// address. Exposed so the encoding can be compared byte for byte.
    pub fn fields(
        env: Env,
        recipient_ref: BytesN<32>,
        destination: MuxedAddress,
        valid_until: u64,
        policy_epoch: u64,
    ) -> Bytes {
        let policy = policy_hash(&env);
        attested_fields(
            &env,
            &recipient_ref,
            &policy,
            policy_epoch,
            &destination,
            valid_until,
        )
    }

    /// The 47 byte message the attester signs for one leg.
    pub fn message(
        env: Env,
        recipient_ref: BytesN<32>,
        destination: MuxedAddress,
        valid_until: u64,
        policy_epoch: u64,
    ) -> Bytes {
        let policy = policy_hash(&env);
        attestation_message(
            &env,
            &recipient_ref,
            &policy,
            policy_epoch,
            &destination,
            valid_until,
        )
    }

    /// The request-time attestation check, one attestation per leg, in leg
    /// order, first failure wins. A bad signature is not a code: the host's
    /// ed25519 verification traps and the whole call fails.
    pub fn verify(
        env: Env,
        recipient_ref: BytesN<32>,
        legs: Vec<Leg>,
        attestations: Vec<Attestation>,
    ) -> Result<(), Error> {
        if legs.len() != attestations.len() {
            return Err(Error::AttestationInvalid);
        }
        let store = env.storage().instance();
        let attester: BytesN<32> = match store.get(&Key::Attester) {
            Some(key) => key,
            None => return Err(Error::ConfigInvalid),
        };
        let policy = policy_hash(&env);
        let epoch: u64 = store.get(&Key::PolicyEpoch).unwrap_or(0);
        let now = env.ledger().timestamp();

        for (leg, att) in legs.iter().zip(attestations.iter()) {
            if now >= att.valid_until {
                return Err(Error::AttestationExpired);
            }
            // valid_until > now here, so the subtraction cannot wrap, and
            // there is no now + MAX to overflow.
            if att.valid_until - now > MAX_ATTESTATION_VALIDITY {
                return Err(Error::AttestationInvalid);
            }
            let signature: BytesN<64> = match att.signature.clone().try_into() {
                Ok(sig) => sig,
                Err(_) => return Err(Error::AttestationInvalid),
            };
            // Checked before the signature, so an attestation from before a
            // policy change gets a code the caller can act on per item
            // instead of a trap that would sink an honest batch.
            if att.policy_epoch != epoch {
                return Err(Error::PolicyChanged);
            }
            let message = attestation_message(
                &env,
                &recipient_ref,
                &policy,
                att.policy_epoch,
                &leg.destination,
                att.valid_until,
            );
            env.crypto().ed25519_verify(&attester, &message, &signature);
        }
        Ok(())
    }
}

fn policy_hash(env: &Env) -> BytesN<32> {
    match env.storage().instance().get(&Key::PolicyHash) {
        Some(hash) => hash,
        None => panic_with_error!(env, Error::ConfigInvalid),
    }
}

fn attested_fields(
    env: &Env,
    recipient_ref: &BytesN<32>,
    policy: &BytesN<32>,
    policy_epoch: u64,
    destination: &MuxedAddress,
    valid_until: u64,
) -> Bytes {
    let mandate = env.current_contract_address().to_xdr(env);
    let dest = destination.address().to_xdr(env);
    let (has_tag, tag) = match destination.id() {
        Some(id) => (1u8, id),
        None => (0u8, 0u64),
    };

    let mut b: Bytes = env.ledger().network_id().into();
    b.push_back(xdr_len(env, &mandate));
    b.append(&mandate);
    b.append(&recipient_ref.clone().into());
    b.append(&policy.clone().into());
    b.extend_from_array(&policy_epoch.to_be_bytes());
    b.extend_from_array(&valid_until.to_be_bytes());
    b.push_back(xdr_len(env, &dest));
    b.append(&dest);
    b.push_back(has_tag);
    b.extend_from_array(&tag.to_be_bytes());
    b
}

fn attestation_message(
    env: &Env,
    recipient_ref: &BytesN<32>,
    policy: &BytesN<32>,
    policy_epoch: u64,
    destination: &MuxedAddress,
    valid_until: u64,
) -> Bytes {
    let fields = attested_fields(
        env,
        recipient_ref,
        policy,
        policy_epoch,
        destination,
        valid_until,
    );
    let mut m = Bytes::from_array(env, PREFIX);
    m.append(&env.crypto().sha256(&fields).to_bytes().into());
    m
}

// Address XDR is 40 bytes for a contract and 44 for an account, so this never
// fails today; it is a check, not a cast, in case a longer address type ever
// appears.
fn xdr_len(env: &Env, b: &Bytes) -> u8 {
    match u8::try_from(b.len()) {
        Ok(n) => n,
        Err(_) => panic_with_error!(env, Error::AttestationInvalid),
    }
}

#[cfg(test)]
mod test;
