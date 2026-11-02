// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The attest step against the reference signer: for every case of the
//! attestation parity fixture, the runner's encoder rebuilds the signed and
//! the checked fields, the message and the signature byte for byte, and its
//! signature tampers reproduce the fixture's damaged signatures.

use ed25519_dalek::SigningKey;
use serde_json::Value;
use std::path::Path;
use vector_runner::attest::{self, AttestInput, Tamper};
use vector_runner::values::{hex, parse_hex, sha256};

fn fixture() -> Value {
    let p =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spikes/attestation-parity/fixture.json");
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn unhex(v: &Value) -> Vec<u8> {
    parse_hex(v.as_str().unwrap()).unwrap()
}

fn arr32(b: &[u8]) -> [u8; 32] {
    b.try_into().unwrap()
}

/// Reads the fields layout back into its inputs, so the encoder can be
/// checked against bytes it did not produce.
fn decode_fields(f: &[u8]) -> AttestInput {
    let mut at = 0usize;
    let mut take = |n: usize| {
        let s = &f[at..at + n];
        at += n;
        s.to_vec()
    };
    let network_id = arr32(&take(32));
    let ml = take(1)[0] as usize;
    let mandate = take(ml);
    let recipient_ref = arr32(&take(32));
    let country_policy_hash = arr32(&take(32));
    let policy_epoch = u64::from_be_bytes(take(8).try_into().unwrap());
    let valid_until = u64::from_be_bytes(take(8).try_into().unwrap());
    let dl = take(1)[0] as usize;
    let destination = take(dl);
    let has_tag = take(1)[0];
    let tag = u64::from_be_bytes(take(8).try_into().unwrap());
    assert!(has_tag <= 1);
    assert_eq!(at, f.len(), "trailing bytes in the fields");
    AttestInput {
        network_id,
        mandate,
        recipient_ref,
        country_policy_hash,
        policy_epoch,
        valid_until,
        destination,
        tag: (has_tag == 1).then_some(tag),
    }
}

fn key_of(fx: &Value, name: &str) -> SigningKey {
    let seed = fx["keys"][name]["seed"].as_str().unwrap();
    let label = seed
        .strip_prefix("sha256(\"")
        .and_then(|s| s.strip_suffix("\")"))
        .unwrap();
    let key = SigningKey::from_bytes(&sha256(label.as_bytes()));
    assert_eq!(
        hex(key.verifying_key().as_bytes()),
        fx["keys"][name]["public_key"].as_str().unwrap()
    );
    key
}

#[test]
fn attest_step_reproduces_every_fixture_case() {
    let fx = fixture();
    let attester = key_of(&fx, "attester");
    let other = key_of(&fx, "other_signer");
    let cases = fx["cases"].as_array().unwrap();
    assert_eq!(
        cases.len(),
        49,
        "the fixture has a different number of cases"
    );

    // The fixture's own damage, mapped to the vector format's tamper kinds
    // where one exists. flip_s, zero, identity and unprefixed signatures are
    // not tamper kinds of the vector format, so for them only the signed bytes
    // are checked.
    let mapped = [
        (63usize, Tamper::ShortSignature),
        (65, Tamper::LongSignature),
        (0, Tamper::Empty),
    ];
    let (mut exact, mut tampered, mut unmapped) = (0, 0, 0);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        for (i, a) in case["attestations"].as_array().unwrap().iter().enumerate() {
            let signed = &a["signed"];
            let fields = unhex(&signed["fields"]);
            let input = decode_fields(&fields);
            assert_eq!(
                attest::fields(&input).unwrap(),
                fields,
                "{name} {i}: fields"
            );
            let (hash, message) = attest::message(&fields);
            assert_eq!(
                hex(&hash),
                signed["fields_sha256"].as_str().unwrap(),
                "{name} {i}"
            );
            assert_eq!(
                hex(&message),
                signed["message"].as_str().unwrap(),
                "{name} {i}"
            );

            // The checked block is what the mandate rebuilds from the
            // request: the case's own network, mandate and leg.
            if !a["checked"].is_null() {
                let leg = &case["legs"][i];
                let checked = AttestInput {
                    network_id: arr32(&unhex(&case["network_id"])),
                    mandate: unhex(&case["mandate_xdr"]),
                    recipient_ref: arr32(&unhex(&case["recipient_ref"])),
                    country_policy_hash: arr32(&unhex(&case["country_policy_hash"])),
                    policy_epoch: a["policy_epoch"].as_str().unwrap().parse().unwrap(),
                    valid_until: a["valid_until"].as_str().unwrap().parse().unwrap(),
                    destination: unhex(&leg["destination_xdr"]),
                    tag: leg["has_tag"]
                        .as_bool()
                        .unwrap()
                        .then(|| leg["tag"].as_str().unwrap().parse().unwrap()),
                };
                assert_eq!(
                    hex(&attest::fields(&checked).unwrap()),
                    a["checked"]["fields"].as_str().unwrap(),
                    "{name} {i}: checked fields"
                );
            }

            let want = unhex(&a["signature"]);
            let honest: Vec<Vec<u8>> = [&attester, &other]
                .iter()
                .map(|k| attest::sign(k, &input).unwrap().signature)
                .collect();
            if honest.contains(&want) {
                exact += 1;
                continue;
            }
            let by_len = mapped
                .iter()
                .find(|(n, _)| *n == want.len())
                .map(|(_, t)| *t);
            let byte_tamper = [Tamper::FlippedBit, Tamper::NonCanonicalS];
            let candidates: Vec<Tamper> = by_len.into_iter().chain(byte_tamper).collect();
            let reproduced = honest.iter().any(|sig| {
                candidates
                    .iter()
                    .any(|t| attest::tamper_signature(sig, *t).unwrap() == want)
            });
            if reproduced {
                tampered += 1;
            } else {
                assert_eq!(want.len(), 64, "{name} {i}: an unexplained signature");
                unmapped += 1;
            }
        }
    }
    println!("{exact} signatures exact, {tampered} tampered as the vector format does, {unmapped} damaged in ways the format has no kind for");
    assert!(exact >= 40, "only {exact} signatures reproduced");
    // Truncated twice, extended, empty, flipped R and S + L.
    assert_eq!(tampered, 6);
    // Flipped S, the zero signature, the identity signature twice and the
    // signature over the bare hash.
    assert_eq!(unmapped, 5);
}

#[test]
fn worked_example_from_the_encoding_spec() {
    let fx = fixture();
    let case = &fx["cases"][0];
    assert_eq!(case["name"], "valid_untagged");
    let a = &case["attestations"][0];
    let input = decode_fields(&unhex(&a["signed"]["fields"]));
    let s = attest::sign(&key_of(&fx, "attester"), &input).unwrap();
    assert_eq!(
        hex(&s.signature),
        "0x043dcca5e9a6dfd41320f9ecf7b4778bf00ff706ac26d865f4c1ff6366a49e0497e63ef43e2a0e6404218231d10d11aa030f954d9eb799ff92f0328e1cae2208"
    );
    assert_eq!(s.message.len(), attest::MESSAGE_BYTES);
}
