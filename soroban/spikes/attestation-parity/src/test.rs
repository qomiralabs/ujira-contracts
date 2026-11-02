// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

extern crate std;

use super::{
    Attestation, AttestationParity, AttestationParityClient, AttestationPolicySet, Error, Leg,
    MESSAGE_BYTES, PREFIX,
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

// The same contract compiled to wasm, so costs include the VM and not only
// the host functions. scripts/check-wasm-fixtures.sh rebuilds it from this
// crate and fails if it drifts.
mod wasm {
    soroban_sdk::contractimport!(file = "fixtures/attestation_parity.wasm");
}

const FIXTURE: &str = include_str!("../../../../spikes/attestation-parity/fixture.json");
const PRIMARY_NETWORK: &str = "Test SDF Network ; September 2015";

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

#[derive(Clone, Copy, PartialEq)]
enum Build {
    Native,
    Wasm,
}

struct Built {
    env: Env,
    contract: Address,
    recipient_ref: BytesN<32>,
    legs: Vec<Leg>,
    attestations: Vec<Attestation>,
}

/// Sets up one fixture case exactly as the fixture describes it and checks
/// every encoding the fields depend on before anything is verified.
fn build(case: &Value, how: Build) -> Built {
    let env = env_at(u64_of(case, "now"), s(case, "network_passphrase"));
    assert_eq!(
        env.ledger().network_id().to_array(),
        arr::<32>(s(case, "network_id")),
        "network id"
    );

    let contract = Address::from_str(&env, s(case, "mandate"));
    let attester = Bytes::from_slice(&env, &unhex(s(case, "attester_public_key")));
    let policy = BytesN::from_array(&env, &arr::<32>(s(case, "country_policy_hash")));
    let epoch = u64_of(case, "policy_epoch");
    match how {
        Build::Native => env.register_at(&contract, AttestationParity, (attester, policy, epoch)),
        Build::Wasm => env.register_at(&contract, wasm::WASM, (attester, policy, epoch)),
    };
    assert_eq!(
        contract.clone().to_xdr(&env),
        Bytes::from_slice(&env, &unhex(s(case, "mandate_xdr"))),
        "mandate bytes"
    );

    let recipient_ref = BytesN::from_array(&env, &arr::<32>(s(case, "recipient_ref")));
    let mut legs = Vec::new(&env);
    for l in case["legs"].as_array().unwrap() {
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
    for a in case["attestations"].as_array().unwrap() {
        attestations.push_back(Attestation {
            valid_until: u64_of(a, "valid_until"),
            policy_epoch: u64_of(a, "policy_epoch"),
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

/// Checks one encoded block from the fixture on its own terms: the hash is
/// the SHA-256 of the fields, and the message is the prefix and that hash.
fn check_encoded(name: &str, e: &Value) -> StdVec<u8> {
    let fields = unhex(s(e, "fields"));
    let hash: [u8; 32] = Sha256::digest(&fields).into();
    assert_eq!(
        hash,
        arr::<32>(s(e, "fields_sha256")),
        "{name}: fields hash"
    );
    let message = unhex(s(e, "message"));
    assert_eq!(
        message.len(),
        MESSAGE_BYTES as usize,
        "{name}: message length"
    );
    assert_eq!(&message[..15], PREFIX, "{name}: message prefix");
    assert_eq!(&message[15..], hash, "{name}: message hash");
    message
}

#[test]
fn every_fixture_case_matches() {
    let fx = fixture();
    let cases = fx["cases"].as_array().unwrap();
    assert!(cases.len() >= 45, "fixture lost cases");
    let mut report = String::new();

    for how in [Build::Native, Build::Wasm] {
        for case in cases {
            let name = s(case, "name");
            let b = build(case, how);
            let client = AttestationParityClient::new(&b.env, &b.contract);

            // Encoding parity: the contract's own fields and message for each
            // leg equal the fixture's, which the off-chain signer also builds.
            let atts = case["attestations"].as_array().unwrap();
            for (i, a) in atts.iter().enumerate() {
                check_encoded(name, &a["signed"]);
                let (Some(leg), false) = (b.legs.get(i as u32), a["checked"].is_null()) else {
                    assert!(
                        a["checked"].is_null(),
                        "{name}: leg {i} and its checked block disagree"
                    );
                    continue;
                };
                let message = check_encoded(name, &a["checked"]);
                let args = (
                    &b.recipient_ref,
                    &leg.destination,
                    &u64_of(a, "valid_until"),
                    &u64_of(a, "policy_epoch"),
                );
                assert_eq!(
                    client.fields(args.0, args.1, args.2, args.3),
                    Bytes::from_slice(&b.env, &unhex(s(&a["checked"], "fields"))),
                    "{name}: fields of leg {i}"
                );
                assert_eq!(
                    client.message(args.0, args.1, args.2, args.3),
                    Bytes::from_slice(&b.env, &message),
                    "{name}: message of leg {i}"
                );
            }

            let expect = s(case, "expect");
            let calls = case["calls"].as_u64().unwrap();
            for call in 0..calls {
                let got = client.try_verify(&b.recipient_ref, &b.legs, &b.attestations);
                let ok = match expect {
                    "ok" => got == Ok(Ok(())),
                    "HOST_TRAP" => got == Err(Err(InvokeError::Abort)),
                    "ATTESTATION_INVALID" => got == Err(Ok(Error::AttestationInvalid)),
                    "ATTESTATION_EXPIRED" => got == Err(Ok(Error::AttestationExpired)),
                    "POLICY_CHANGED" => got == Err(Ok(Error::PolicyChanged)),
                    other => panic!("{name}: unknown expectation {other}"),
                };
                assert!(ok, "{name} call {call}: expected {expect}, got {got:?}");
            }
            if how == Build::Native {
                report.push_str(&format!("{name}: {expect}\n"));
            }
        }
    }
    println!("{report}");
}

/// A trap is not a contract error, so the host's own error is checked too:
/// every trap case must fail in ed25519 verification and nowhere else.
#[test]
fn trap_cases_fail_in_ed25519_verification() {
    let fx = fixture();
    let mut seen = 0;
    for case in fx["cases"].as_array().unwrap() {
        if s(case, "expect") != "HOST_TRAP" {
            continue;
        }
        seen += 1;
        let name = s(case, "name");
        let b = build(case, Build::Native);
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
            "{name}: {got:?}"
        );
        let events = format!("{:?}", b.env.host().get_diagnostic_events().unwrap());
        assert!(
            events.contains("Error(Crypto(InvalidInput))"),
            "{name}: not a crypto error"
        );
        assert!(
            events.contains("failed ED25519 verification"),
            "{name}: trapped somewhere other than ed25519 verification"
        );
    }
    assert!(seen >= 15, "fixture lost its trap cases");
}

/// The keys are public by construction: each one is the SHA-256 of the label
/// the fixture names, and an independent ed25519 implementation agrees with
/// every signature the fixture expects to verify.
#[test]
fn keys_come_from_their_labels() {
    let fx = fixture();
    let mut signers = StdVec::new();
    for name in ["attester", "other_signer"] {
        let k = &fx["keys"][name];
        let seed = s(k, "seed");
        let label = seed
            .strip_prefix("sha256(\"")
            .and_then(|r| r.strip_suffix("\")"))
            .unwrap_or_else(|| panic!("{name}: seed is not sha256 of a label"));
        assert!(label.starts_with("UJIRA/TEST-ONLY/"), "{name}: label");
        let sk = SigningKey::from_bytes(&Sha256::digest(label.as_bytes()).into());
        assert_eq!(
            sk.verifying_key().to_bytes(),
            arr::<32>(s(k, "public_key")),
            "{name}: public key"
        );
        signers.push(sk);
    }
    assert_ne!(signers[0].verifying_key(), signers[1].verifying_key());

    let attester = signers[0].verifying_key();
    for case in fx["cases"].as_array().unwrap() {
        if s(case, "expect") != "ok" {
            continue;
        }
        for a in case["attestations"].as_array().unwrap() {
            let message = check_encoded(s(case, "name"), &a["checked"]);
            let sig = Signature::from_bytes(&arr::<64>(s(a, "signature")));
            assert!(
                attester.verify_strict(&message, &sig).is_ok(),
                "{}: signature",
                s(case, "name")
            );
            // ed25519 is deterministic, so signing again gives the same bytes.
            assert_eq!(signers[0].sign(&message).to_bytes(), sig.to_bytes());
        }
    }
}

#[test]
fn constructor_emits_policy() {
    let env = env_at(1_767_225_600, PRIMARY_NETWORK);
    let key = arr::<32>(s(&fixture()["keys"]["attester"], "public_key"));
    let policy = BytesN::from_array(&env, &[7u8; 32]);
    let id = env.register(
        AttestationParity,
        (Bytes::from_slice(&env, &key), policy.clone(), 5u64),
    );
    let event = AttestationPolicySet {
        attester: BytesN::from_array(&env, &key),
        country_policy_hash: policy,
        policy_epoch: 5,
    };
    let expected = soroban_sdk::Event::to_xdr(&event, &env, &id);
    assert_eq!(env.events().all(), [expected]);
}

fn register_with_key(env: &Env, key: &[u8]) {
    env.register(
        AttestationParity,
        (
            Bytes::from_slice(env, key),
            BytesN::from_array(env, &[0u8; 32]),
            0u64,
        ),
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #34)")]
fn constructor_rejects_zero_key() {
    let env = env_at(1_767_225_600, PRIMARY_NETWORK);
    register_with_key(&env, &[0u8; 32]);
}

#[test]
#[should_panic(expected = "Error(Contract, #34)")]
fn constructor_rejects_short_key() {
    let env = env_at(1_767_225_600, PRIMARY_NETWORK);
    register_with_key(&env, &[1u8; 31]);
}

#[test]
#[should_panic(expected = "Error(Contract, #34)")]
fn constructor_rejects_long_key() {
    let env = env_at(1_767_225_600, PRIMARY_NETWORK);
    register_with_key(&env, &[1u8; 33]);
}

/// Keys the contract cannot tell are bad, because it only checks length and
/// zero. The host's strict verification must still refuse them; the operator
/// is meant to catch them before they are ever configured.
#[test]
fn malformed_attester_keys_trap() {
    let fx = fixture();
    let base = fx["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| s(c, "name") == "valid_untagged")
        .unwrap();
    let message = unhex(s(&base["attestations"][0]["checked"], "message"));

    // The identity key with R = identity and S = 0 passes a lax,
    // cofactorless check for any message.
    let mut identity = [0u8; 32];
    identity[0] = 1;
    let mut weak_sig = [0u8; 64];
    weak_sig[..32].copy_from_slice(&identity);
    let lax = VerifyingKey::from_bytes(&identity).unwrap();
    assert!(
        lax.verify(&message, &Signature::from_bytes(&weak_sig))
            .is_ok(),
        "the lax check accepts it"
    );

    // y = p is a non-canonical encoding of y = 0.
    let mut non_canonical = [0xffu8; 32];
    non_canonical[0] = 0xed;
    non_canonical[31] = 0x7f;
    let real_sig = arr::<64>(s(&base["attestations"][0], "signature"));

    for (label, key, sig) in [
        ("small-order key", identity, weak_sig),
        ("non-canonical key", non_canonical, real_sig),
    ] {
        let mut case = base.clone();
        case["attester_public_key"] = Value::String(format!("0x{}", hexs(&key)));
        case["attestations"][0]["signature"] = Value::String(format!("0x{}", hexs(&sig)));
        let b = build(&case, Build::Native);
        let client = AttestationParityClient::new(&b.env, &b.contract);
        assert_eq!(
            client.try_verify(&b.recipient_ref, &b.legs, &b.attestations),
            Err(Err(InvokeError::Abort)),
            "{label}"
        );
    }
}

fn hexs(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Metered cost of every fixture case on the compiled wasm, plus the
/// marginal cost of a verified leg. The numbers in the spike note come from
/// this test; run it with --nocapture to see them.
#[test]
fn wasm_cost_per_case() {
    let fx = fixture();
    let cases = fx["cases"].as_array().unwrap();
    let mut table = String::new();
    let mut cost_of = std::collections::BTreeMap::new();
    for case in cases {
        let name = s(case, "name");
        let b = build(case, Build::Wasm);
        let client = AttestationParityClient::new(&b.env, &b.contract);
        b.env.cost_estimate().budget().reset_default();
        let _ = client.try_verify(&b.recipient_ref, &b.legs, &b.attestations);
        let budget = b.env.cost_estimate().budget();
        let (cpu, mem) = (budget.cpu_instruction_cost(), budget.memory_bytes_cost());
        table.push_str(&format!(
            "| `{name}` | {} | {} | {cpu} | {mem} |\n",
            case["tamper"].as_str().unwrap_or("none"),
            s(case, "expect"),
        ));
        cost_of.insert(String::from(name), cpu);
    }
    println!("{table}");

    let one = cost_of["valid_untagged"];
    let two = cost_of["valid_two_legs_same_address_two_tags"];
    let none = cost_of["missing_attestation"];
    println!(
        "wasm: 1 leg {one}, 2 legs {two}, count refusal {none}, marginal leg {}",
        two - one
    );
    assert!(two > one && one > none);
    // A trap happens inside the same verification an honest leg runs, so it
    // can never cost more than the leg would have.
    assert!(cost_of["flipped_s_bit"] <= one);
}

/// The attester really signed the bare hash, so this case tests the verifier's
/// prefix and not a bad signature. The fixture's `wrong_chain` differs from
/// what it presents in the network ID alone.
#[test]
fn unprefixed_and_wrong_chain_cases_are_what_they_claim() {
    let fx = fixture();
    let cases = fx["cases"].as_array().unwrap();
    let case = |name: &str| {
        cases
            .iter()
            .find(|c| s(c, "name") == name)
            .unwrap_or_else(|| panic!("the fixture has no case {name}"))
    };

    let unprefixed = case("unprefixed_message");
    assert_eq!(s(unprefixed, "expect"), "HOST_TRAP");
    let attestation = &unprefixed["attestations"][0];
    let key = VerifyingKey::from_bytes(&arr::<32>(s(unprefixed, "attester_public_key"))).unwrap();
    let signature = Signature::from_bytes(&arr::<64>(s(attestation, "signature")));
    let hash = unhex(s(&attestation["signed"], "fields_sha256"));
    assert_eq!(hash.len(), 32);
    key.verify(&hash, &signature)
        .expect("the attester signed the bare hash");
    assert!(key
        .verify(&unhex(s(&attestation["checked"], "message")), &signature)
        .is_err());

    let chain = case("wrong_chain");
    assert_eq!(s(chain, "expect"), "HOST_TRAP");
    let attestation = &chain["attestations"][0];
    let signed = unhex(s(&attestation["signed"], "fields"));
    let checked = unhex(s(&attestation["checked"], "fields"));
    assert_ne!(signed[..32], checked[..32], "the network IDs are the same");
    assert_eq!(signed[32..], checked[32..], "more than the network differs");
}
