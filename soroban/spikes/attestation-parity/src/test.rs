// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

extern crate std;

use super::{
    scalars_in_range, Attestation, AttestationParity, AttestationParityClient,
    AttestationPolicySet, Error, Leg, HALF_ORDER, ORDER,
};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde_json::Value;
use sha2::{Digest, Sha256};
use soroban_sdk::{
    testutils::{EnvTestConfig, Events, Ledger},
    xdr::{ScErrorType, ToXdr},
    Address, Bytes, BytesN, Env, IntoVal, InvokeError, MuxedAddress, Symbol, Vec,
};
use std::{format, println, string::String, vec::Vec as StdVec};

const FIXTURE: &str = include_str!("../../../../spikes/attestation-parity/fixture.json");

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("fixture is valid JSON")
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key]
        .as_str()
        .unwrap_or_else(|| panic!("missing string {key}"))
}

fn unhex(h: &str) -> StdVec<u8> {
    let h = h.strip_prefix("0x").unwrap_or(h);
    assert!(h.len().is_multiple_of(2), "odd hex length");
    (0..h.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
        .collect()
}

fn arr<const N: usize>(h: &str) -> [u8; N] {
    unhex(h)
        .try_into()
        .unwrap_or_else(|_| panic!("expected {N} bytes"))
}

fn u64_of(v: &Value, key: &str) -> u64 {
    s(v, key).parse().unwrap()
}

fn env_at(now: u64, passphrase: &str) -> Env {
    // One env per fixture case; per-env snapshot files would only duplicate
    // the fixture.
    let env = Env::new_with_config(EnvTestConfig {
        capture_snapshot_at_drop: false,
    });
    let network_id: [u8; 32] = Sha256::digest(passphrase.as_bytes()).into();
    env.ledger().with_mut(|li| {
        li.timestamp = now;
        li.network_id = network_id;
    });
    env
}

struct Built {
    env: Env,
    contract: Address,
    recipient_ref: BytesN<32>,
    legs: Vec<Leg>,
    attestations: Vec<Attestation>,
}

/// Sets up one fixture case exactly as the fixture describes it and checks
/// every encoding the digest depends on before anything is verified.
fn build(case: &Value) -> Built {
    let c = &case["soroban"];
    let env = env_at(u64_of(case, "now"), s(c, "network_passphrase"));
    assert_eq!(
        env.ledger().network_id().to_array(),
        arr::<32>(s(c, "chain_tag")),
        "chain tag"
    );

    let contract = Address::from_str(&env, s(c, "mandate"));
    let attester = BytesN::from_array(&env, &arr::<65>(s(case, "attester_public_key")));
    let policy = BytesN::from_array(&env, &arr::<32>(s(case, "country_policy_hash")));
    env.register_at(&contract, AttestationParity, (attester, policy));
    assert_eq!(
        contract.clone().to_xdr(&env),
        Bytes::from_slice(&env, &unhex(s(c, "mandate_xdr"))),
        "mandate bytes"
    );

    let recipient_ref = BytesN::from_array(&env, &arr::<32>(s(case, "recipient_ref")));
    let mut legs = Vec::new(&env);
    for l in c["legs"].as_array().unwrap() {
        let destination = MuxedAddress::from_str(&env, s(l, "destination"));
        assert_eq!(
            destination.address().to_xdr(&env),
            Bytes::from_slice(&env, &unhex(s(l, "destination_xdr"))),
            "destination bytes"
        );
        let expected_tag = if l["has_tag"].as_bool().unwrap() {
            Some(u64_of(l, "tag"))
        } else {
            None
        };
        assert_eq!(destination.id(), expected_tag, "destination tag");
        legs.push_back(Leg {
            destination,
            amount: s(l, "amount").parse().unwrap(),
        });
    }

    let mut attestations = Vec::new(&env);
    for a in c["attestations"].as_array().unwrap() {
        attestations.push_back(Attestation {
            valid_until: u64_of(a, "valid_until"),
            signature: Bytes::from_slice(&env, &unhex(s(a, "signature"))),
        });
    }
    Built {
        env,
        contract,
        recipient_ref,
        legs,
        attestations,
    }
}

#[test]
fn every_fixture_case_matches() {
    let fx = fixture();
    let cases = fx["cases"].as_array().unwrap();
    assert!(cases.len() >= 40, "fixture lost cases");
    let mut report = String::new();

    for case in cases {
        let name = s(case, "name");
        let b = build(case);
        let client = AttestationParityClient::new(&b.env, &b.contract);

        // Digest parity: the contract's own digest for each leg equals the
        // fixture's, which the off-chain signer and the Celo suite also use.
        let atts = case["soroban"]["attestations"].as_array().unwrap();
        for (i, a) in atts.iter().enumerate() {
            if let (Some(d), Some(leg)) = (a["digest"].as_str(), b.legs.get(i as u32)) {
                let got = client.digest(
                    &b.recipient_ref,
                    &leg.destination,
                    &u64_of(a, "valid_until"),
                );
                assert_eq!(got.to_array(), arr::<32>(d), "{name}: digest of leg {i}");
            }
        }

        let expect = s(case, "expect");
        let abort = case["soroban_host_abort"].as_bool().unwrap();
        let calls = case["calls"].as_u64().unwrap();
        for call in 0..calls {
            let got = client.try_verify(&b.recipient_ref, &b.legs, &b.attestations);
            let ok = match (expect, abort) {
                ("ok", false) => got == Ok(Ok(())),
                (_, true) => got == Err(Err(InvokeError::Abort)),
                ("ATTESTATION_INVALID", false) => got == Err(Ok(Error::AttestationInvalid)),
                ("ATTESTATION_EXPIRED", false) => got == Err(Ok(Error::AttestationExpired)),
                (other, _) => panic!("{name}: unknown expectation {other}"),
            };
            assert!(
                ok,
                "{name} call {call}: expected {expect} (host abort {abort}), got {got:?}"
            );
        }
        report.push_str(&format!(
            "{name}: {expect}{}\n",
            if abort { " (host abort)" } else { "" }
        ));
    }
    println!("{report}");
}

/// A trap is not a contract error, so the host's own error is checked too:
/// the abort cases must fail as invalid crypto input and nothing else.
#[test]
fn host_abort_cases_fail_in_recovery() {
    let fx = fixture();
    let mut seen = 0;
    for case in fx["cases"].as_array().unwrap() {
        if !case["soroban_host_abort"].as_bool().unwrap() {
            continue;
        }
        seen += 1;
        let b = build(case);
        let args = (
            b.recipient_ref.clone(),
            b.legs.clone(),
            b.attestations.clone(),
        )
            .into_val(&b.env);
        let got = b.env.try_invoke_contract::<(), soroban_sdk::Error>(
            &b.contract,
            &Symbol::new(&b.env, "verify"),
            args,
        );
        // A native test contract surfaces the trap to its caller as a
        // generic context error; the diagnostic log keeps the real cause.
        assert!(
            matches!(got, Err(Ok(e)) if !e.is_type(ScErrorType::Contract)),
            "{}: {got:?}",
            s(case, "name")
        );
        let events = format!("{:?}", b.env.host().get_diagnostic_events().unwrap());
        assert!(
            events.contains("Error(Crypto(InvalidInput))"),
            "{}: not a crypto error",
            s(case, "name")
        );
        assert!(
            events.contains("ECDSA-secp256k1 signature recovery failed"),
            "{}: wrong trap",
            s(case, "name")
        );
    }
    assert!(seen >= 2, "fixture lost its host abort cases");
}

#[test]
fn half_order_boundary() {
    let one = {
        let mut b = [0u8; 32];
        b[31] = 1;
        b
    };
    let mut half_plus_one = HALF_ORDER;
    half_plus_one[31] += 1;
    let mut order_minus_one = ORDER;
    order_minus_one[31] -= 1;

    assert!(scalars_in_range(&one, &HALF_ORDER), "s = n/2 is low");
    assert!(
        !scalars_in_range(&one, &half_plus_one),
        "s = n/2 + 1 is high"
    );
    assert!(
        scalars_in_range(&order_minus_one, &one),
        "r = n - 1 is in range"
    );
    assert!(!scalars_in_range(&ORDER, &one), "r = n is out of range");
    assert!(!scalars_in_range(&[0; 32], &one), "r = 0");
    assert!(!scalars_in_range(&one, &[0; 32]), "s = 0");
}

#[test]
fn constructor_emits_policy() {
    let env = env_at(1_767_225_600, "Test SDF Network ; September 2015");
    let attester = BytesN::from_array(
        &env,
        &arr::<65>(s(&fixture()["keys"]["attester"], "public_key")),
    );
    let policy = BytesN::from_array(&env, &[7u8; 32]);
    let id = env.register(AttestationParity, (attester.clone(), policy.clone()));
    let event = AttestationPolicySet {
        attester,
        country_policy_hash: policy,
    };
    let expected = soroban_sdk::Event::to_xdr(&event, &env, &id);
    assert_eq!(env.events().all(), [expected]);
}

#[test]
#[should_panic(expected = "Error(Contract, #34)")]
fn constructor_rejects_compressed_key() {
    let env = env_at(1_767_225_600, "Test SDF Network ; September 2015");
    let mut key = [0u8; 65];
    key[0] = 0x02;
    env.register(
        AttestationParity,
        (
            BytesN::from_array(&env, &key),
            BytesN::from_array(&env, &[0u8; 32]),
        ),
    );
}

/// Cost of one verification, from the host budget. Native test execution
/// meters host functions but not the contract's own wasm, so the wasm figure
/// comes from testnet simulation; this pins the host-side floor.
#[test]
fn cost_per_leg() {
    let fx = fixture();
    let cases = fx["cases"].as_array().unwrap();
    let pick = |name: &str| cases.iter().find(|c| s(c, "name") == name).unwrap();

    let measure = |case: &Value| {
        let b = build(case);
        let client = AttestationParityClient::new(&b.env, &b.contract);
        b.env.cost_estimate().budget().reset_default();
        client.verify(&b.recipient_ref, &b.legs, &b.attestations);
        let budget = b.env.cost_estimate().budget();
        (budget.cpu_instruction_cost(), budget.memory_bytes_cost())
    };
    let (cpu1, mem1) = measure(pick("valid_untagged"));
    let (cpu2, mem2) = measure(pick("valid_two_legs_same_address_two_tags"));
    println!("soroban native: 1 leg cpu {cpu1} mem {mem1}; 2 legs cpu {cpu2} mem {mem2}; marginal leg cpu {}", cpu2 - cpu1);
    assert!(cpu2 > cpu1);
}

/// The ed25519 alternative, for comparison only. The host uses strict
/// verification, so the classic edge cases must all trap.
#[test]
fn ed25519_edge_cases() {
    let env = env_at(1_767_225_600, "Test SDF Network ; September 2015");
    let id = env.register(
        AttestationParity,
        (
            BytesN::from_array(
                &env,
                &arr::<65>(s(&fixture()["keys"]["attester"], "public_key")),
            ),
            BytesN::from_array(&env, &[0u8; 32]),
        ),
    );
    let client = AttestationParityClient::new(&env, &id);

    let sk = SigningKey::from_bytes(&Sha256::digest(b"UJIRA/TEST-ONLY/S18/ed25519").into());
    let pk = sk.verifying_key().to_bytes();
    let msg = arr::<32>(s(
        &fixture()["cases"][0]["soroban"]["attestations"][0],
        "digest",
    ));
    let sig = sk.sign(&msg).to_bytes();

    let call = |pk: [u8; 32], m: &[u8], sig: [u8; 64]| {
        client.try_verify_ed25519(
            &BytesN::from_array(&env, &pk),
            &Bytes::from_slice(&env, m),
            &BytesN::from_array(&env, &sig),
        )
    };

    env.cost_estimate().budget().reset_default();
    assert_eq!(call(pk, &msg, sig), Ok(Ok(())), "valid");
    println!(
        "soroban native ed25519 verify cpu {}",
        env.cost_estimate().budget().cpu_instruction_cost()
    );

    let mut other = msg;
    other[0] ^= 1;
    assert!(call(pk, &other, sig).is_err(), "wrong message");

    // S + L: same scalar mod L, non-canonical encoding. Malleable if accepted.
    let l: [u8; 32] = [
        0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde,
        0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10,
    ];
    let mut high = sig;
    let mut carry = 0u16;
    for i in 0..32 {
        let v = u16::from(high[32 + i]) + u16::from(l[i]) + carry;
        high[32 + i] = v as u8;
        carry = v >> 8;
    }
    assert!(call(pk, &msg, high).is_err(), "non-canonical S");

    // Identity public key with R = identity, S = 0 passes a cofactorless,
    // non-strict check for any message. Strict verification refuses the
    // small-order key.
    let mut identity = [0u8; 32];
    identity[0] = 1;
    let mut weak_sig = [0u8; 64];
    weak_sig[..32].copy_from_slice(&identity);
    let lax = VerifyingKey::from_bytes(&identity).unwrap();
    assert!(
        lax.verify(&msg, &Signature::from_bytes(&weak_sig)).is_ok(),
        "the forgery works without strict checks"
    );
    assert!(call(identity, &msg, weak_sig).is_err(), "small-order key");

    // y = p is a non-canonical encoding of y = 0.
    let mut non_canonical = [0xffu8; 32];
    non_canonical[0] = 0xed;
    non_canonical[31] = 0x7f;
    assert!(call(non_canonical, &msg, sig).is_err(), "non-canonical key");

    let mut flipped = sig;
    flipped[0] ^= 1;
    assert!(call(pk, &msg, flipped).is_err(), "damaged R");
}
