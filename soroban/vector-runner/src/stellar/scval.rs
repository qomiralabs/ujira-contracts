// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Building and reading the XDR values the contracts see.
//!
//! Arguments are built directly as `ScVal`, from the interface described in
//! `tests/README.md`, so the runner needs no Rust types from the contract
//! and checks the contract's interface rather than sharing it. A
//! `#[contracttype]` struct is a map keyed by its field names in sorted
//! order, an `Option` is the value or void, and a `MuxedAddress` is an
//! account, muxed account or contract address.

use crate::Result;
use serde_json::{Map, Value};
use soroban_sdk::xdr::{
    AccountId, ContractId, Hash, Int128Parts, Limits, MuxedEd25519Account, PublicKey, ScAddress,
    ScBytes, ScMap, ScMapEntry, ScSymbol, ScVal, ScVec, Uint256, WriteXdr,
};

pub fn account(pk: [u8; 32]) -> ScAddress {
    ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(pk))))
}

pub fn contract(id: [u8; 32]) -> ScAddress {
    ScAddress::Contract(ContractId(Hash(id)))
}

pub fn muxed(pk: [u8; 32], id: u64) -> ScAddress {
    ScAddress::MuxedAccount(MuxedEd25519Account {
        id,
        ed25519: Uint256(pk),
    })
}

/// The XDR bytes of the address as an `ScVal`, which is what
/// `Address::to_xdr` hashes and what the runner keys addresses by.
pub fn address_xdr(a: &ScAddress) -> Result<Vec<u8>> {
    ScVal::Address(a.clone())
        .to_xdr(Limits::none())
        .map_err(|e| format!("cannot encode an address: {e}"))
}

pub fn symbol(s: &str) -> Result<ScVal> {
    Ok(ScVal::Symbol(ScSymbol(
        s.try_into().map_err(|_| format!("{s:?} is not a symbol"))?,
    )))
}

pub fn bytes(b: &[u8]) -> Result<ScVal> {
    Ok(ScVal::Bytes(ScBytes(
        b.to_vec()
            .try_into()
            .map_err(|_| String::from("bytes too long for XDR"))?,
    )))
}

pub fn vec(items: Vec<ScVal>) -> Result<ScVal> {
    Ok(ScVal::Vec(Some(ScVec(
        items
            .try_into()
            .map_err(|_| String::from("list too long for XDR"))?,
    ))))
}

/// A struct: a map from field name to value, sorted by name as the host
/// requires.
pub fn record(mut fields: Vec<(&str, ScVal)>) -> Result<ScVal> {
    fields.sort_by(|a, b| a.0.cmp(b.0));
    let mut entries = Vec::with_capacity(fields.len());
    for (k, v) in fields {
        entries.push(ScMapEntry {
            key: symbol(k)?,
            val: v,
        });
    }
    Ok(ScVal::Map(Some(ScMap(
        entries
            .try_into()
            .map_err(|_| String::from("map too long for XDR"))?,
    ))))
}

pub fn i128_val(v: i128) -> ScVal {
    ScVal::I128(Int128Parts {
        hi: (v >> 64) as i64,
        lo: v as u64,
    })
}

pub fn i128_of(p: &Int128Parts) -> i128 {
    (i128::from(p.hi) << 64) | i128::from(p.lo)
}

/// How a decoded value's raw bytes and addresses are written back in the
/// vector's own forms.
pub trait Names {
    /// The actor or reserved name of an address, if it has one.
    fn name_of(&self, a: &ScAddress) -> Option<String>;
    /// A 32-byte value as a label the vector uses, or hex.
    fn bytes32(&self, b: &[u8; 32]) -> String;
    /// An attester key as a label, `{ "account_key": actor }`, or hex.
    fn attester(&self, b: &[u8]) -> Value;
}

fn address_name(a: &ScAddress, names: &dyn Names) -> String {
    match names.name_of(a) {
        Some(n) => n,
        None => format!("unknown address {a:?}"),
    }
}

/// Decodes a value into the vector's written form. `key` is the field the
/// value sits in, which decides how a status, a mode, an error code, a
/// destination or an attester is written.
pub fn decode(
    v: &ScVal,
    key: &str,
    names: &dyn Names,
    errors: &dyn Fn(u32) -> Option<String>,
) -> Result<Value> {
    Ok(match v {
        ScVal::Void => Value::Null,
        ScVal::Bool(b) => Value::Bool(*b),
        ScVal::U32(n) => match key {
            "status" => Value::String(
                crate::spec::status_name(*n)
                    .ok_or_else(|| format!("{n} is not a payout status"))?
                    .to_string(),
            ),
            "recipient_mode" => Value::String(
                crate::spec::mode_name(*n)
                    .ok_or_else(|| format!("{n} is not a recipient mode"))?
                    .to_string(),
            ),
            "code" if *n == 0 => Value::from(0),
            "code" => Value::String(errors(*n).ok_or_else(|| format!("{n} is not an error code"))?),
            _ => Value::from(*n),
        },
        ScVal::I32(n) => Value::from(*n),
        ScVal::U64(n) => Value::String(n.to_string()),
        ScVal::I64(n) => Value::String(n.to_string()),
        ScVal::I128(p) => Value::String(i128_of(p).to_string()),
        ScVal::U128(p) => Value::String(((u128::from(p.hi) << 64) | u128::from(p.lo)).to_string()),
        ScVal::Bytes(b) => {
            let raw: &[u8] = b.0.as_slice();
            if key == "attester" {
                names.attester(raw)
            } else if let Ok(arr) = <[u8; 32]>::try_from(raw) {
                Value::String(names.bytes32(&arr))
            } else {
                Value::String(crate::values::hex(raw))
            }
        }
        ScVal::String(s) => Value::String(s.0.to_utf8_string_lossy()),
        ScVal::Symbol(s) => Value::String(s.0.to_utf8_string_lossy()),
        ScVal::Vec(Some(items)) => Value::Array(
            items
                .0
                .iter()
                .map(|i| decode(i, key, names, errors))
                .collect::<Result<Vec<_>>>()?,
        ),
        ScVal::Vec(None) => Value::Array(Vec::new()),
        ScVal::Map(Some(m)) => {
            let mut out = Map::new();
            for e in m.0.iter() {
                let k = match &e.key {
                    ScVal::Symbol(s) => s.0.to_utf8_string_lossy(),
                    ScVal::String(s) => s.0.to_utf8_string_lossy(),
                    other => return Err(format!("a map key that is not a name: {other:?}")),
                };
                let val = decode(&e.val, &k, names, errors)?;
                out.insert(k, val);
            }
            Value::Object(out)
        }
        ScVal::Map(None) => Value::Object(Map::new()),
        ScVal::Address(a) => match a {
            ScAddress::MuxedAccount(m) => serde_json::json!({
                "address": address_name(&account(m.ed25519.0), names),
                "tag": m.id.to_string(),
            }),
            other if key == "destination" => serde_json::json!({
                "address": address_name(other, names),
                "tag": null,
            }),
            other => Value::String(address_name(other, names)),
        },
        other => return Err(format!("a value the interface never uses: {other:?}")),
    })
}
