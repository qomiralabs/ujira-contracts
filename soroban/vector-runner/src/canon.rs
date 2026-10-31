// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The one form two values are compared in.
//!
//! A vector writes a 32-byte value as a label or as hex, and an attester as a
//! label, raw bytes or an actor's key. A trace may write the same bytes
//! either way too. Before any comparison both sides are rewritten so that
//! every 32-byte value and every attester is `0x` hex of the bytes it stands
//! for. Labels and actor keys are pure functions of their names, so this
//! needs neither the vector nor the chain.

use crate::values::{actor_key, bytes32, hex, is_label, label_key, parse_hex};
use crate::Result;
use serde_json::{Map, Value};

/// Keys whose values are 32-byte values wherever they appear.
pub const BYTES32_KEYS: [&str; 6] = [
    "payout_id",
    "recipient_ref",
    "task_ref",
    "tenant_ref",
    "country_policy_hash",
    "salt",
];

/// Maps keyed by 32-byte values.
pub const BYTES32_MAPS: [&str; 2] = ["payouts", "daily"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ctx {
    Plain,
    Bytes32,
    Attester,
}

pub fn ctx_for_key(key: &str) -> Ctx {
    if key == "attester" {
        Ctx::Attester
    } else if BYTES32_KEYS.contains(&key) {
        Ctx::Bytes32
    } else {
        Ctx::Plain
    }
}

/// The canonical form of `value`.
pub fn canon(value: &Value) -> Result<Value> {
    canon_in(value, Ctx::Plain)
}

pub fn canon_in(value: &Value, ctx: Ctx) -> Result<Value> {
    match (ctx, value) {
        (Ctx::Attester, Value::Null) => Ok(Value::Null),
        (Ctx::Attester, Value::String(s)) => Ok(Value::String(attester_hex(s)?)),
        (Ctx::Attester, Value::Object(o)) => match o.get("account_key").and_then(Value::as_str) {
            Some(actor) if o.len() == 1 => Ok(Value::String(hex(actor_key(actor)
                .verifying_key()
                .as_bytes()))),
            _ => Err(format!("{value} is not an attester")),
        },
        (Ctx::Bytes32, Value::String(s)) => Ok(Value::String(hex(&bytes32(s)?))),
        // A label means 32 bytes wherever a key is not expected, so an ID in
        // a list canonicalises without knowing its field.
        (Ctx::Plain, Value::String(s)) if is_label(s) => Ok(Value::String(hex(&bytes32(s)?))),
        (_, Value::Array(items)) => items
            .iter()
            .map(|v| canon_in(v, ctx))
            .collect::<Result<Vec<_>>>()
            .map(Value::Array),
        (_, Value::Object(o)) => {
            let mut out = Map::new();
            for (k, v) in o {
                let inner = if BYTES32_MAPS.contains(&k.as_str()) {
                    canon_keyed_map(v)?
                } else {
                    canon_in(v, ctx_for_key(k))?
                };
                out.insert(k.clone(), inner);
            }
            Ok(Value::Object(out))
        }
        _ => Ok(value.clone()),
    }
}

/// A map keyed by 32-byte values, such as `payouts`, with its keys in hex.
fn canon_keyed_map(v: &Value) -> Result<Value> {
    match v {
        Value::Object(o) => {
            let mut out = Map::new();
            for (k, inner) in o {
                out.insert(canon_key(k)?, canon(inner)?);
            }
            Ok(Value::Object(out))
        }
        other => canon(other),
    }
}

pub fn canon_key(key: &str) -> Result<String> {
    Ok(hex(&bytes32(key)?))
}

/// An attester as hex of its key bytes: a label names a test key and stands
/// for its public key; raw bytes stand for themselves.
fn attester_hex(s: &str) -> Result<String> {
    if is_label(s) {
        return Ok(hex(label_key(s)?.verifying_key().as_bytes()));
    }
    Ok(hex(&parse_hex(s)?))
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::values::label_bytes;
    use serde_json::json;

    #[test]
    fn labels_and_hex_meet_in_one_form() {
        let p1 = hex(&label_bytes("#p1").unwrap());
        let a = canon(&json!({"payouts": {"#p1": {"status": "APPROVED"}}, "x": ["#p1"]})).unwrap();
        let b =
            canon(&json!({"payouts": {p1.clone(): {"status": "APPROVED"}}, "x": [p1]})).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn an_attester_label_is_a_public_key_not_the_label_bytes() {
        let key = label_key("#attester").unwrap();
        let got = canon(&json!({"attester": "#attester"})).unwrap();
        assert_eq!(got["attester"], hex(key.verifying_key().as_bytes()));
        assert_ne!(got["attester"], hex(&label_bytes("#attester").unwrap()));
        let by_actor = canon(&json!({"attester": {"account_key": "agent"}})).unwrap();
        assert_eq!(
            by_actor["attester"],
            hex(actor_key("agent").verifying_key().as_bytes())
        );
    }

    #[test]
    fn actor_names_and_amounts_stay_as_written() {
        let v = json!({"by": "guardian", "amount": "100", "tag": null});
        assert_eq!(canon(&v).unwrap(), v);
    }
}
