// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

extern crate std;

use super::MandateError;
use serde_json::Value;
use soroban_sdk::{
    xdr::{Limits, ReadXdr, ScErrorCode, ScErrorType, ScSpecEntry},
    Error, InvokeError,
};
use std::{string::String, vec::Vec};

// Read at compile time, so a table edit without the matching enum edit fails
// these tests as well as the CI check.
const ERRORS_JSON: &str = include_str!("../../../../spec/errors.json");

/// (name, code) for every entry in the table.
fn table() -> Vec<(String, u32)> {
    let entries: Value = serde_json::from_str(ERRORS_JSON).expect("errors.json is valid JSON");
    entries
        .as_array()
        .expect("errors.json is a list")
        .iter()
        .map(|entry| {
            let name = entry["name"].as_str().expect("every entry has a name");
            let code = entry["code"].as_u64().expect("every entry has a code");
            let code = u32::try_from(code).expect("every code fits in a u32");
            (String::from(name), code)
        })
        .collect()
}

/// (variant name, code) for every variant, decoded from the contract spec the
/// macro generates. The compiler writes that list, so a variant cannot be left
/// out of it the way it could be from a list typed into this file.
fn variants() -> Vec<(String, u32)> {
    let entry = ScSpecEntry::from_xdr(MandateError::spec_xdr(), Limits::none())
        .expect("the generated spec decodes");
    let ScSpecEntry::UdtErrorEnumV0(spec) = entry else {
        panic!("MandateError is not in the spec as an error enum");
    };
    spec.cases
        .iter()
        .map(|case| {
            let name = case.name.to_utf8_string().expect("variant names are ASCII");
            (name, case.value)
        })
        .collect()
}

/// AgentRemoved becomes AGENT_REMOVED.
fn upper_snake(name: &str) -> String {
    let mut out = String::new();
    let mut after_lower = false;
    for c in name.chars() {
        if c.is_ascii_uppercase() && after_lower {
            out.push('_');
        }
        after_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        out.push(c.to_ascii_uppercase());
    }
    out
}

#[test]
fn upper_snake_splits_on_each_capital() {
    assert_eq!(upper_snake("Unauthorized"), "UNAUTHORIZED");
    assert_eq!(upper_snake("PayoutIdExists"), "PAYOUT_ID_EXISTS");
    // An acronym is not split, so it can never match a table name.
    assert_eq!(upper_snake("PayoutIDExists"), "PAYOUT_IDEXISTS");
}

#[test]
fn every_variant_matches_the_spec_table() {
    let mut want = table();
    want.sort_by_key(|&(_, code)| code);
    let mut have: Vec<(String, u32)> = variants()
        .into_iter()
        .map(|(name, code)| (upper_snake(&name), code))
        .collect();
    have.sort_by_key(|&(_, code)| code);
    let missing: Vec<_> = want.iter().filter(|entry| !have.contains(entry)).collect();
    let extra: Vec<_> = have.iter().filter(|entry| !want.contains(entry)).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "in the table but not the enum: {missing:?}; in the enum but not the table: {extra:?}"
    );
    // Also catches an entry listed twice, which the two filters above miss.
    assert_eq!(have, want);
}

#[test]
fn every_variant_keeps_its_code_through_the_host() {
    for (name, code) in variants() {
        let error = Error::from_contract_error(code);
        let variant = MandateError::try_from(error)
            .unwrap_or_else(|_| panic!("code {code} does not convert back to {name}"));
        assert_eq!(variant as u32, code, "{name}");
        assert_eq!(Error::from(variant), error, "{name}");
        assert_eq!(
            InvokeError::from(variant),
            InvokeError::Contract(code),
            "{name}"
        );
        assert_eq!(
            MandateError::try_from(InvokeError::Contract(code)),
            Ok(variant),
            "{name}"
        );
    }
}

#[test]
fn other_errors_never_read_as_a_mandate_error() {
    // Below, between and above the codes in use.
    for code in [0, 13, 91, u32::MAX] {
        let error = Error::from_contract_error(code);
        assert_eq!(MandateError::try_from(error), Err(error), "code {code}");
    }
    // A host failure, such as a refused authorization, keeps its own type.
    let host = Error::from_type_and_code(ScErrorType::Auth, ScErrorCode::InvalidAction);
    assert_eq!(MandateError::try_from(host), Err(host));
    assert_eq!(
        MandateError::try_from(InvokeError::Abort),
        Err(InvokeError::Abort)
    );
}
