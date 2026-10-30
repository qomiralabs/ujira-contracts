// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Recipient attestations: the version 2 message, the ed25519 signature over
//! it, and the tamper kinds of the vector format.
//!
//! The byte layout must equal the reference signer's for every case of the
//! attestation parity fixture; the self tests check that byte for byte. The
//! encoder takes address bytes, not addresses, so it is the same code for
//! every chain that hashes `Address::to_xdr` bytes.

use crate::Result;
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};

pub const PREFIX: &[u8; 15] = b"UJIRA/ATTEST/V2";
pub const MESSAGE_BYTES: usize = 47;
pub const SIGNATURE_BYTES: usize = 64;

/// The order of the ed25519 group, little endian.
const GROUP_ORDER_LE: [u8; 32] = [
    0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10,
];

/// Everything the attester signs for one leg.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttestInput {
    pub network_id: [u8; 32],
    /// The mandate's `Address::to_xdr` bytes.
    pub mandate: Vec<u8>,
    pub recipient_ref: [u8; 32],
    pub country_policy_hash: [u8; 32],
    pub policy_epoch: u64,
    pub valid_until: u64,
    /// The base address of the destination, as `Address::to_xdr` bytes.
    pub destination: Vec<u8>,
    pub tag: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed {
    pub fields: Vec<u8>,
    pub fields_sha256: [u8; 32],
    pub message: [u8; MESSAGE_BYTES],
    pub signature: Vec<u8>,
}

/// What a request carries for one leg.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attestation {
    pub valid_until: u64,
    pub policy_epoch: u64,
    pub signature: Vec<u8>,
}

fn length_prefixed(out: &mut Vec<u8>, bytes: &[u8], what: &str) -> Result<()> {
    let len = u8::try_from(bytes.len()).map_err(|_| format!("{what} is longer than 255 bytes"))?;
    out.push(len);
    out.extend_from_slice(bytes);
    Ok(())
}

pub fn fields(input: &AttestInput) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(220);
    out.extend_from_slice(&input.network_id);
    length_prefixed(&mut out, &input.mandate, "the mandate address")?;
    out.extend_from_slice(&input.recipient_ref);
    out.extend_from_slice(&input.country_policy_hash);
    out.extend_from_slice(&input.policy_epoch.to_be_bytes());
    out.extend_from_slice(&input.valid_until.to_be_bytes());
    length_prefixed(&mut out, &input.destination, "the destination address")?;
    // No tag is has_tag 0 with tag 0, so it can never equal tag 0.
    let (has_tag, tag) = match input.tag {
        Some(t) => (1u8, t),
        None => (0u8, 0u64),
    };
    out.push(has_tag);
    out.extend_from_slice(&tag.to_be_bytes());
    Ok(out)
}

pub fn message(fields: &[u8]) -> ([u8; 32], [u8; MESSAGE_BYTES]) {
    let hash: [u8; 32] = Sha256::digest(fields).into();
    let mut m = [0u8; MESSAGE_BYTES];
    m[..PREFIX.len()].copy_from_slice(PREFIX);
    m[PREFIX.len()..].copy_from_slice(&hash);
    (hash, m)
}

pub fn sign(key: &SigningKey, input: &AttestInput) -> Result<Signed> {
    let f = fields(input)?;
    let (fields_sha256, message) = message(&f);
    let signature = key.sign(&message).to_bytes().to_vec();
    Ok(Signed {
        fields: f,
        fields_sha256,
        message,
        signature,
    })
}

/// Flips the lowest bit of `bytes[index]`.
pub fn flip_low_bit(bytes: &mut [u8], index: usize) -> Result<()> {
    let b = bytes
        .get_mut(index)
        .ok_or_else(|| format!("no byte {index} to flip"))?;
    *b ^= 1;
    Ok(())
}

fn flip_last(bytes: &mut [u8]) -> Result<()> {
    let last = bytes
        .len()
        .checked_sub(1)
        .ok_or_else(|| String::from("nothing to flip"))?;
    flip_low_bit(bytes, last)
}

/// S + L in 32 bytes little endian: the same scalar mod L, written the one
/// way a strict verifier refuses.
pub fn add_group_order(s: &[u8; 32]) -> Result<[u8; 32]> {
    let mut out = [0u8; 32];
    let mut carry = 0u16;
    for i in 0..32 {
        let sum = u16::from(s[i]) + u16::from(GROUP_ORDER_LE[i]) + carry;
        out[i] = (sum & 0xff) as u8;
        carry = sum >> 8;
    }
    if carry != 0 {
        return Err(String::from("S + L does not fit in 32 bytes"));
    }
    Ok(out)
}

/// The tamper kinds of the vector format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tamper {
    WrongSigner,
    WrongChain,
    WrongMandate,
    WrongDestination,
    WrongTag,
    WrongPolicy,
    WrongRef,
    WrongValidUntil,
    FlippedBit,
    NonCanonicalS,
    Empty,
    Truncated,
    ShortSignature,
    LongSignature,
    Extended,
}

impl Tamper {
    pub fn parse(name: &str) -> Result<Tamper> {
        Ok(match name {
            "wrong_signer" => Tamper::WrongSigner,
            "wrong_chain" => Tamper::WrongChain,
            "wrong_mandate" => Tamper::WrongMandate,
            "wrong_destination" => Tamper::WrongDestination,
            "wrong_tag" => Tamper::WrongTag,
            "wrong_policy" => Tamper::WrongPolicy,
            "wrong_ref" => Tamper::WrongRef,
            "wrong_valid_until" => Tamper::WrongValidUntil,
            "flipped_bit" => Tamper::FlippedBit,
            "non_canonical_s" => Tamper::NonCanonicalS,
            "empty" => Tamper::Empty,
            "truncated" => Tamper::Truncated,
            "short_signature" => Tamper::ShortSignature,
            "long_signature" => Tamper::LongSignature,
            "extended" => Tamper::Extended,
            other => return Err(format!("unknown tamper kind {other:?}")),
        })
    }
}

/// Changes what is signed, for the tamper kinds that sign something other
/// than what the mandate will check.
pub fn tamper_input(input: &mut AttestInput, t: Tamper) -> Result<()> {
    match t {
        Tamper::WrongChain => flip_last(&mut input.network_id),
        Tamper::WrongMandate => flip_last(&mut input.mandate),
        Tamper::WrongDestination => flip_last(&mut input.destination),
        Tamper::WrongPolicy => flip_last(&mut input.country_policy_hash),
        Tamper::WrongRef => flip_last(&mut input.recipient_ref),
        Tamper::WrongValidUntil => {
            input.valid_until ^= 1;
            Ok(())
        }
        Tamper::WrongTag => {
            input.tag = Some(match input.tag {
                None => 0,
                Some(t) => t ^ 1,
            });
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Damages a finished signature, for the tamper kinds that do.
pub fn tamper_signature(sig: &[u8], t: Tamper) -> Result<Vec<u8>> {
    let mut out = sig.to_vec();
    match t {
        Tamper::FlippedBit => flip_low_bit(&mut out, 0)?,
        Tamper::NonCanonicalS => {
            let s: [u8; 32] = sig
                .get(32..64)
                .and_then(|s| s.try_into().ok())
                .ok_or_else(|| String::from("the signature is not 64 bytes"))?;
            out[32..64].copy_from_slice(&add_group_order(&s)?);
        }
        Tamper::Empty => out.clear(),
        Tamper::Truncated => out.truncate(32),
        Tamper::ShortSignature => out.truncate(63),
        Tamper::LongSignature => out.push(0),
        Tamper::Extended => out.extend_from_slice(&[0u8; 64]),
        _ => {}
    }
    Ok(out)
}

/// One attest step: `signer` signs `input`, or `wrong_signer` does for that
/// tamper, and the tamper is applied. The attestation carries the honest
/// `valid_until` and `policy_epoch` whatever was signed.
pub fn attest(
    signer: &SigningKey,
    wrong_signer: &SigningKey,
    input: &AttestInput,
    tamper: Option<Tamper>,
) -> Result<(Signed, Attestation)> {
    let mut signed_input = input.clone();
    let mut key = signer;
    if let Some(t) = tamper {
        tamper_input(&mut signed_input, t)?;
        if t == Tamper::WrongSigner {
            key = wrong_signer;
        }
    }
    let mut signed = sign(key, &signed_input)?;
    if let Some(t) = tamper {
        signed.signature = tamper_signature(&signed.signature, t)?;
    }
    let att = Attestation {
        valid_until: input.valid_until,
        policy_epoch: input.policy_epoch,
        signature: signed.signature.clone(),
    };
    Ok((signed, att))
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn group_order_addition_carries() {
        let mut s = [0u8; 32];
        s[0] = 0x20;
        let got = add_group_order(&s).unwrap();
        assert_eq!(got[0], 0x0d);
        assert_eq!(got[1], 0xd4);
        assert_eq!(got[31], 0x10);
    }

    #[test]
    fn signature_tampers_have_their_lengths() {
        let sig = [7u8; 64];
        let len = |t| tamper_signature(&sig, t).unwrap().len();
        assert_eq!(len(Tamper::Empty), 0);
        assert_eq!(len(Tamper::Truncated), 32);
        assert_eq!(len(Tamper::ShortSignature), 63);
        assert_eq!(len(Tamper::LongSignature), 65);
        assert_eq!(len(Tamper::Extended), 128);
        assert_eq!(len(Tamper::FlippedBit), 64);
        assert_eq!(tamper_signature(&sig, Tamper::FlippedBit).unwrap()[0], 6);
    }

    #[test]
    fn wrong_tag_marks_a_missing_tag_as_zero() {
        let mut input = AttestInput {
            network_id: [0; 32],
            mandate: vec![1],
            recipient_ref: [0; 32],
            country_policy_hash: [0; 32],
            policy_epoch: 0,
            valid_until: 10,
            destination: vec![2],
            tag: None,
        };
        tamper_input(&mut input, Tamper::WrongTag).unwrap();
        assert_eq!(input.tag, Some(0));
        tamper_input(&mut input, Tamper::WrongTag).unwrap();
        assert_eq!(input.tag, Some(1));
        tamper_input(&mut input, Tamper::WrongValidUntil).unwrap();
        assert_eq!(input.valid_until, 11);
    }
}
