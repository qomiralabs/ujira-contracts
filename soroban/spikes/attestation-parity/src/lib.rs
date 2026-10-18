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
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Leg {
    pub destination: MuxedAddress,
    pub amount: i128,
}

/// The signature is `Bytes`, not `BytesN<65>`, so a short or long signature
/// reaches the contract and gets a contract error code, instead of failing
/// argument decoding before any contract code runs.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attestation {
    pub valid_until: u64,
    pub signature: Bytes,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttestationPolicySet {
    pub attester: BytesN<65>,
    pub country_policy_hash: BytesN<32>,
}

#[contracttype]
enum Key {
    Attester,
    PolicyHash,
}

const DOMAIN: &[u8; 15] = b"UJIRA/ATTEST/V1";
const MAX_ATTESTATION_VALIDITY: u64 = 86_400;

// secp256k1 group order n and floor(n / 2), big-endian.
const ORDER: [u8; 32] = [
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe,
    0xba, 0xae, 0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c, 0xd0, 0x36, 0x41, 0x41,
];
const HALF_ORDER: [u8; 32] = [
    0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0x5d, 0x57, 0x6e, 0x73, 0x57, 0xa4, 0x50, 0x1d, 0xdf, 0xe9, 0x2f, 0x46, 0x68, 0x1b, 0x20, 0xa0,
];
const ZERO: [u8; 32] = [0; 32];

/// The host's recover traps on out-of-range r or s, high s and recovery ids
/// above 3. Rejecting those here first is what keeps the error code a
/// contract error the caller can match on. Fixed-width big-endian arrays
/// compare like the numbers they encode.
pub fn scalars_in_range(r: &[u8; 32], s: &[u8; 32]) -> bool {
    *r != ZERO && *r < ORDER && *s != ZERO && *s <= HALF_ORDER
}

#[contract]
pub struct AttestationParity;

#[contractimpl]
impl AttestationParity {
    pub fn __constructor(env: Env, attester: BytesN<65>, country_policy_hash: BytesN<32>) {
        if attester.get(0) != Some(0x04) {
            panic_with_error!(&env, Error::ConfigInvalid);
        }
        let store = env.storage().instance();
        store.set(&Key::Attester, &attester);
        store.set(&Key::PolicyHash, &country_policy_hash);
        AttestationPolicySet {
            attester,
            country_policy_hash,
        }
        .publish(&env);
    }

    /// The digest the attester signs for one leg, computed from this
    /// contract's own network and address.
    pub fn digest(
        env: Env,
        recipient_ref: BytesN<32>,
        destination: MuxedAddress,
        valid_until: u64,
    ) -> BytesN<32> {
        let policy: BytesN<32> = env.storage().instance().get(&Key::PolicyHash).unwrap();
        attestation_digest(&env, &recipient_ref, &policy, &destination, valid_until)
    }

    /// The request-time attestation check, one attestation per leg, in leg
    /// order, first failure wins.
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
        let attester: BytesN<65> = store.get(&Key::Attester).unwrap();
        let policy: BytesN<32> = store.get(&Key::PolicyHash).unwrap();
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
            if att.signature.len() != 65 {
                return Err(Error::AttestationInvalid);
            }
            let v = att.signature.get_unchecked(64);
            if v != 27 && v != 28 {
                return Err(Error::AttestationInvalid);
            }
            let mut r = [0u8; 32];
            let mut s = [0u8; 32];
            att.signature.slice(0..32).copy_into_slice(&mut r);
            att.signature.slice(32..64).copy_into_slice(&mut s);
            if !scalars_in_range(&r, &s) {
                return Err(Error::AttestationInvalid);
            }

            let digest = keccak_digest(
                &env,
                &recipient_ref,
                &policy,
                &leg.destination,
                att.valid_until,
            );
            let rs: BytesN<64> = att.signature.slice(0..64).try_into().unwrap();
            // Still traps when r is not the x coordinate of a curve point or
            // the result is the point at infinity. Only a submitter that made
            // up the signature can hit this, and the whole call fails.
            let recovered = env
                .crypto()
                .secp256k1_recover(&digest, &rs, u32::from(v - 27));
            if recovered != attester {
                return Err(Error::AttestationInvalid);
            }
        }
        Ok(())
    }

    /// Measurement only: the cost of the ed25519 alternative on the same
    /// digest. Traps on any failure; the host gives no way to catch it.
    pub fn verify_ed25519(env: Env, public_key: BytesN<32>, message: Bytes, signature: BytesN<64>) {
        env.crypto()
            .ed25519_verify(&public_key, &message, &signature);
    }
}

fn digest_preimage(
    env: &Env,
    recipient_ref: &BytesN<32>,
    policy: &BytesN<32>,
    destination: &MuxedAddress,
    valid_until: u64,
) -> Bytes {
    let mandate = env.current_contract_address().to_xdr(env);
    let dest = destination.address().to_xdr(env);
    let (has_tag, tag) = match destination.id() {
        Some(id) => (1u8, id),
        None => (0u8, 0u64),
    };

    let mut b = Bytes::from_array(env, DOMAIN);
    b.append(&env.ledger().network_id().into());
    b.append(&recipient_ref.clone().into());
    b.append(&policy.clone().into());
    b.extend_from_array(&valid_until.to_be_bytes());
    b.push_back(xdr_len(env, &mandate));
    b.append(&mandate);
    b.push_back(xdr_len(env, &dest));
    b.append(&dest);
    b.push_back(has_tag);
    b.extend_from_array(&tag.to_be_bytes());
    b
}

fn keccak_digest(
    env: &Env,
    recipient_ref: &BytesN<32>,
    policy: &BytesN<32>,
    destination: &MuxedAddress,
    valid_until: u64,
) -> soroban_sdk::crypto::Hash<32> {
    env.crypto().keccak256(&digest_preimage(
        env,
        recipient_ref,
        policy,
        destination,
        valid_until,
    ))
}

fn attestation_digest(
    env: &Env,
    recipient_ref: &BytesN<32>,
    policy: &BytesN<32>,
    destination: &MuxedAddress,
    valid_until: u64,
) -> BytesN<32> {
    keccak_digest(env, recipient_ref, policy, destination, valid_until).to_bytes()
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
