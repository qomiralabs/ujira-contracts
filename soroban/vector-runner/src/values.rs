// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The written forms of vector values, and the fixed bytes and keys that
//! actor names and labels stand for. Everything here is a pure function of
//! its input, so two runners, or two traces, always resolve a name the same
//! way.

use crate::Result;
use ed25519_dalek::SigningKey;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// A label `#name` is the SHA-256 of this prefix followed by the name.
pub const LABEL_PREFIX: &str = "UJIRA/TEST-ONLY/vector/";

/// An actor's key seed is the SHA-256 of this prefix followed by its name.
pub const ACTOR_PREFIX: &str = "ujira-test-actor:";

/// Names with a fixed meaning in every vector.
pub const RESERVED: [&str; 8] = [
    "mandate",
    "factory",
    "asset",
    "other_token",
    "fee_address",
    "zero",
    "zero_account",
    "zero_contract",
];

/// The two tokens a vector can name.
pub const TOKENS: [&str; 2] = ["asset", "other_token"];

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

pub fn is_label(s: &str) -> bool {
    match s.strip_prefix('#') {
        Some(name) => is_name(name),
        None => false,
    }
}

/// Lower case letters, digits and `_`, starting with a letter: the form of
/// actor names, labels, vector IDs and attestation names.
pub fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// The 32 bytes a label stands for.
pub fn label_bytes(label: &str) -> Result<[u8; 32]> {
    match label.strip_prefix('#') {
        Some(name) if is_name(name) => {
            let mut text = String::from(LABEL_PREFIX);
            text.push_str(name);
            Ok(sha256(text.as_bytes()))
        }
        _ => Err(format!("{label:?} is not a label")),
    }
}

/// The ed25519 test key a label names where a key is expected. Its seed is
/// the label's bytes, so the key is public by construction.
pub fn label_key(label: &str) -> Result<SigningKey> {
    Ok(SigningKey::from_bytes(&label_bytes(label)?))
}

pub fn actor_seed(name: &str) -> [u8; 32] {
    let mut text = String::from(ACTOR_PREFIX);
    text.push_str(name);
    sha256(text.as_bytes())
}

/// The ed25519 key behind an account actor's address.
pub fn actor_key(name: &str) -> SigningKey {
    SigningKey::from_bytes(&actor_seed(name))
}

pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(2 + bytes.len() * 2);
    out.push_str("0x");
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// `0x` and an even number of lower case hex digits.
pub fn parse_hex(s: &str) -> Result<Vec<u8>> {
    let digits = s
        .strip_prefix("0x")
        .ok_or_else(|| format!("{s:?} does not start with 0x"))?;
    if digits.len() % 2 != 0 {
        return Err(format!("{s:?} has an odd number of hex digits"));
    }
    let mut out = Vec::with_capacity(digits.len() / 2);
    let raw = digits.as_bytes();
    for pair in raw.chunks(2) {
        let hi = hex_digit(pair[0]).ok_or_else(|| format!("{s:?} is not lower case hex"))?;
        let lo = hex_digit(pair[1]).ok_or_else(|| format!("{s:?} is not lower case hex"))?;
        out.push(hi << 4 | lo);
    }
    Ok(out)
}

fn hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

/// A 32-byte value written as a label or as `0x` and 64 hex digits.
pub fn bytes32(s: &str) -> Result<[u8; 32]> {
    if s.starts_with('#') {
        return label_bytes(s);
    }
    let raw = parse_hex(s)?;
    raw.try_into()
        .map_err(|_| format!("{s:?} is not exactly 32 bytes"))
}

pub fn str_of<'a>(v: &'a Value, what: &str) -> Result<&'a str> {
    v.as_str()
        .ok_or_else(|| format!("{what} is not a string: {v}"))
}

/// A u64 written as a decimal string.
pub fn parse_u64(v: &Value, what: &str) -> Result<u64> {
    let s = str_of(v, what)?;
    s.parse::<u64>()
        .map_err(|_| format!("{what} {s:?} is not a u64"))
}

/// An amount written as a decimal string. Stellar amounts are i128, so a
/// value outside it cannot be expressed here at all; check-vectors restricts
/// such vectors to the chains that can.
pub fn parse_amount(v: &Value, what: &str) -> Result<i128> {
    let s = str_of(v, what)?;
    s.parse::<i128>()
        .map_err(|_| format!("{what} {s:?} is not an amount an i128 can hold"))
}

/// A small integer written as a JSON integer.
pub fn parse_u32(v: &Value, what: &str) -> Result<u32> {
    v.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| format!("{what} is not a u32: {v}"))
}

pub fn is_reserved(name: &str) -> bool {
    RESERVED.contains(&name)
}

pub fn is_token(name: &str) -> bool {
    TOKENS.contains(&name)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn labels_are_the_sha256_of_their_prefixed_name() {
        let got = label_bytes("#p1").unwrap();
        assert_eq!(got, sha256(b"UJIRA/TEST-ONLY/vector/p1"));
        assert_ne!(got, label_bytes("#p2").unwrap());
        assert!(label_bytes("p1").is_err());
        assert!(label_bytes("#P1").is_err());
    }

    #[test]
    fn hex_round_trips_and_refuses_upper_case() {
        let raw = [0u8, 1, 0xab, 0xff];
        assert_eq!(parse_hex(&hex(&raw)).unwrap(), raw);
        assert!(parse_hex("0xAB").is_err());
        assert!(parse_hex("ab").is_err());
        assert!(parse_hex("0xa").is_err());
        assert_eq!(parse_hex("0x").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn amounts_cover_the_whole_i128_range() {
        let min = Value::String("-170141183460469231731687303715884105728".into());
        assert_eq!(parse_amount(&min, "a").unwrap(), i128::MIN);
        let above = Value::String("170141183460469231731687303715884105728".into());
        assert!(parse_amount(&above, "a").is_err());
        assert!(parse_amount(&Value::from(5), "a").is_err());
    }
}
