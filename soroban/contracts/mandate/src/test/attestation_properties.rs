// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The attestation encoding under random inputs.
//!
//! The hand picked cases of the parity fixture pin the layout at a few points.
//! These properties close the gaps between them: every bound field really
//! changes the message, the contract agrees with an independent encoder on
//! every input, and every tamper traps.
//!
//! The reference encoder below builds the bytes by hand from the documented
//! layout and shares no code with the contract, the SDK's XDR or the host's
//! SHA-256. A mandate is always a contract, so the mandate address is only
//! ever generated as one; the destination is an account or a contract, and
//! only an account can carry a tag.
//!
//! The runs are repeatable: a fixed seed unless `UJIRA_ATTESTATION_SEED` says
//! otherwise, and 1,000 cases unless `UJIRA_ATTESTATION_CASES` says otherwise.

extern crate std;

use crate::attest;
use crate::storage::Policy;
use crate::types::Attestation;
use crate::MandateError;
use ed25519_dalek::{Signer, SigningKey};
use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use serde_json::Value;
use sha2::{Digest, Sha256};
use soroban_sdk::testutils::{EnvTestConfig, Ledger as _};
use soroban_sdk::xdr::{
    ContractId, Hash, MuxedEd25519Account, PublicKey, ScAddress, ScVal, Uint256,
};
use soroban_sdk::{contract, contractimpl, Address, Bytes, BytesN, Env, MuxedAddress, TryFromVal};
use std::string::String;
use std::vec::Vec;

const PREFIX: &[u8] = b"UJIRA/ATTEST/V2";
const MESSAGE_BYTES: usize = 47;
const TRANSACTION_HASH_BYTES: usize = 32;
const SCVAL_ADDRESS: [u8; 4] = [0, 0, 0, 18];
const DEFAULT_SEED: u64 = 0x5552_4553_5456_3032;
const DEFAULT_CASES: u32 = 1_000;

/// Runs the contract's own encoder and checks with the contract's address as
/// the mandate, which only a call into a contract at that address can do.
#[contract]
pub struct Probe;

#[contractimpl]
impl Probe {
    pub fn message(
        env: Env,
        recipient_ref: BytesN<32>,
        country_policy_hash: BytesN<32>,
        policy_epoch: u64,
        destination: MuxedAddress,
        valid_until: u64,
    ) -> Result<Bytes, MandateError> {
        attest::message(
            &env,
            &recipient_ref,
            &country_policy_hash,
            policy_epoch,
            &destination,
            valid_until,
        )
    }

    pub fn check(
        env: Env,
        attester: BytesN<32>,
        policy: Policy,
        recipient_ref: BytesN<32>,
        destination: MuxedAddress,
        attestation: Attestation,
        now: u64,
    ) -> Result<u64, MandateError> {
        attest::check_leg(
            &env,
            &attester,
            &policy,
            &recipient_ref,
            &destination,
            &attestation,
            now,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Dest {
    Account([u8; 32]),
    Contract([u8; 32]),
}

/// Every field the attester signs.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Fields {
    network: [u8; 32],
    mandate: [u8; 32],
    recipient: [u8; 32],
    country: [u8; 32],
    epoch: u64,
    valid_until: u64,
    dest: Dest,
    tag: Option<u64>,
}

fn fields() -> impl Strategy<Value = Fields> {
    let dest = prop_oneof![
        (any::<[u8; 32]>(), any::<Option<u64>>()).prop_map(|(k, tag)| (Dest::Account(k), tag)),
        any::<[u8; 32]>().prop_map(|h| (Dest::Contract(h), None)),
    ];
    // A tag of 0 and the extremes are the values most likely to be mistaken
    // for no tag or to lose a byte, so they get drawn on purpose.
    let dest = prop_oneof![
        3 => dest,
        1 => any::<[u8; 32]>().prop_map(|k| (Dest::Account(k), Some(0))),
        1 => any::<[u8; 32]>().prop_map(|k| (Dest::Account(k), Some(u64::MAX))),
    ];
    let numbers = (
        prop_oneof![Just(0u64), Just(u64::MAX), any::<u64>()],
        prop_oneof![Just(1u64), Just(u64::MAX), any::<u64>()],
    );
    (
        any::<[u8; 32]>(),
        any::<[u8; 32]>(),
        any::<[u8; 32]>(),
        any::<[u8; 32]>(),
        numbers,
        dest,
    )
        .prop_map(
            |(network, mandate, recipient, country, (epoch, valid_until), (dest, tag))| Fields {
                network,
                mandate,
                recipient,
                country,
                epoch,
                valid_until,
                dest,
                tag,
            },
        )
}

// The reference encoder.

fn address_xdr(contract: bool, key: &[u8; 32]) -> Vec<u8> {
    let mut out = SCVAL_ADDRESS.to_vec();
    if contract {
        out.extend_from_slice(&[0, 0, 0, 1]);
    } else {
        // SC_ADDRESS_TYPE_ACCOUNT, then PUBLIC_KEY_TYPE_ED25519.
        out.extend_from_slice(&[0; 8]);
    }
    out.extend_from_slice(key);
    out
}

fn reference_fields(f: &Fields) -> Vec<u8> {
    let mandate = address_xdr(true, &f.mandate);
    let dest = match &f.dest {
        Dest::Account(k) => address_xdr(false, k),
        Dest::Contract(h) => address_xdr(true, h),
    };
    let mut out = Vec::new();
    out.extend_from_slice(&f.network);
    out.push(u8::try_from(mandate.len()).unwrap());
    out.extend_from_slice(&mandate);
    out.extend_from_slice(&f.recipient);
    out.extend_from_slice(&f.country);
    out.extend_from_slice(&f.epoch.to_be_bytes());
    out.extend_from_slice(&f.valid_until.to_be_bytes());
    out.push(u8::try_from(dest.len()).unwrap());
    out.extend_from_slice(&dest);
    out.push(u8::from(f.tag.is_some()));
    out.extend_from_slice(&f.tag.unwrap_or(0).to_be_bytes());
    out
}

fn reference_message(f: &Fields) -> Vec<u8> {
    let mut out = PREFIX.to_vec();
    out.extend_from_slice(&Sha256::digest(reference_fields(f)));
    out
}

// The contract's side.

fn contract_address(env: &Env, hash: &[u8; 32]) -> Address {
    Address::try_from_val(env, &ScAddress::Contract(ContractId(Hash(*hash)))).unwrap()
}

fn destination(env: &Env, f: &Fields) -> MuxedAddress {
    let sc = match (&f.dest, f.tag) {
        (Dest::Account(k), None) => ScAddress::Account(soroban_sdk::xdr::AccountId(
            PublicKey::PublicKeyTypeEd25519(Uint256(*k)),
        )),
        (Dest::Account(k), Some(id)) => ScAddress::MuxedAccount(MuxedEd25519Account {
            id,
            ed25519: Uint256(*k),
        }),
        (Dest::Contract(h), None) => ScAddress::Contract(ContractId(Hash(*h))),
        (Dest::Contract(_), Some(_)) => panic!("a contract destination carries no tag"),
    };
    MuxedAddress::try_from_val(env, &ScVal::Address(sc)).unwrap()
}

fn new_env() -> Env {
    Env::new_with_config(EnvTestConfig {
        capture_snapshot_at_drop: false,
    })
}

/// A probe at the mandate's address on the case's network.
fn probe<'a>(env: &'a Env, f: &Fields) -> ProbeClient<'a> {
    env.ledger().set_network_id(f.network);
    // One case makes many calls into fresh addresses, and only the encoding
    // is under test here, never its cost.
    env.cost_estimate().budget().reset_unlimited();
    let at = contract_address(env, &f.mandate);
    ProbeClient::new(env, &env.register_at(&at, Probe, ()))
}

fn contract_message(env: &Env, f: &Fields) -> Vec<u8> {
    let client = probe(env, f);
    let msg = client.message(
        &BytesN::from_array(env, &f.recipient),
        &BytesN::from_array(env, &f.country),
        &f.epoch,
        destination(env, f),
        &f.valid_until,
    );
    let mut out = std::vec![0u8; msg.len() as usize];
    msg.copy_into_slice(&mut out);
    out
}

/// The field a flip changed, for the failure message.
type Flip = (&'static str, Fields);

fn flip_bit(bytes: &[u8; 32], bit: usize) -> [u8; 32] {
    let mut out = *bytes;
    out[(bit / 8) % 32] ^= 1 << (bit % 8);
    out
}

/// One single change per bound field, the tag presence flag among them.
fn flips(f: &Fields, bit: usize) -> Vec<Flip> {
    let mask = 1u64 << (bit % 64);
    let mut out: Vec<Flip> = Vec::new();
    let mut add = |name, change: &dyn Fn(&mut Fields)| {
        let mut g = f.clone();
        change(&mut g);
        out.push((name, g));
    };
    add("network", &|g| g.network = flip_bit(&f.network, bit));
    add("mandate", &|g| g.mandate = flip_bit(&f.mandate, bit));
    add("recipient", &|g| g.recipient = flip_bit(&f.recipient, bit));
    add("country", &|g| g.country = flip_bit(&f.country, bit));
    add("epoch", &|g| g.epoch = f.epoch ^ mask);
    add("valid_until", &|g| g.valid_until = f.valid_until ^ mask);
    match f.dest {
        Dest::Account(k) => {
            add("destination", &|g| {
                g.dest = Dest::Account(flip_bit(&k, bit));
            });
            // The same 32 bytes as the other kind of address.
            add("destination kind", &|g| {
                g.dest = Dest::Contract(k);
                g.tag = None;
            });
            match f.tag {
                // No tag and a tag of 0 must sign differently.
                None => add("tag presence", &|g| g.tag = Some(0)),
                Some(t) => {
                    add("tag presence", &|g| g.tag = None);
                    add("tag", &|g| g.tag = Some(t ^ mask));
                }
            }
        }
        Dest::Contract(h) => {
            add("destination", &|g| {
                g.dest = Dest::Contract(flip_bit(&h, bit));
            });
            add("destination kind", &|g| g.dest = Dest::Account(h));
        }
    }
    out
}

fn seed() -> [u8; 32] {
    let seed = std::env::var("UJIRA_ATTESTATION_SEED")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_SEED);
    let mut out = [0u8; 32];
    out[..8].copy_from_slice(&seed.to_be_bytes());
    out
}

fn cases() -> u32 {
    std::env::var("UJIRA_ATTESTATION_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_CASES)
}

fn runner() -> TestRunner {
    TestRunner::new_with_rng(
        Config {
            cases: cases(),
            failure_persistence: None,
            ..Config::default()
        },
        TestRng::from_seed(RngAlgorithm::ChaCha, &seed()),
    )
}

fn must_hold<T>(result: Result<(), proptest::test_runner::TestError<T>>)
where
    T: core::fmt::Debug,
{
    if let Err(e) = result {
        panic!("{e}");
    }
}

/// The message is always 47 bytes under its prefix, so it can never be
/// mistaken for a 32-byte transaction hash, and changing any single bound
/// field changes it.
#[test]
fn attestation_encoding_properties() {
    must_hold(runner().run(&(fields(), 0usize..256), |(f, bit)| {
        let env = new_env();
        let message = contract_message(&env, &f);
        prop_assert_eq!(message.len(), MESSAGE_BYTES);
        prop_assert_ne!(message.len(), TRANSACTION_HASH_BYTES);
        prop_assert_eq!(&message[..PREFIX.len()], PREFIX);
        for (name, changed) in flips(&f, bit) {
            prop_assert_ne!(
                &message,
                &contract_message(&env, &changed),
                "changing {} left the message as it was",
                name
            );
        }
        Ok(())
    }));
}

/// A signature from the attester passes the mandate's check. A signature from
/// another key, one flipped bit of the attester's, and the attester's over
/// any changed field all trap.
#[test]
fn attestation_encoding_properties_tampers_trap() {
    let attester =
        SigningKey::from_bytes(&Sha256::digest(b"UJIRA/TEST-ONLY/S18/attester-ed25519").into());
    let other =
        SigningKey::from_bytes(&Sha256::digest(b"UJIRA/TEST-ONLY/S18/other-signer-ed25519").into());
    must_hold(runner().run(&(fields(), 0usize..512), |(mut f, bit)| {
        // A window the check accepts, so the signature is what decides.
        f.valid_until = f.valid_until.max(1);
        let now = f.valid_until - 1;
        let env = new_env();
        let client = probe(&env, &f);
        let policy = Policy {
            attester: Some(Bytes::from_array(&env, attester.verifying_key().as_bytes())),
            country_policy_hash: BytesN::from_array(&env, &f.country),
            policy_epoch: f.epoch,
            max_fee_bps: 100,
        };
        let key = BytesN::from_array(&env, attester.verifying_key().as_bytes());
        let recipient = BytesN::from_array(&env, &f.recipient);
        let signed = attester.sign(&reference_message(&f)).to_bytes();
        let attestation = |signature: &[u8; 64]| Attestation {
            valid_until: f.valid_until,
            policy_epoch: f.epoch,
            signature: Bytes::from_array(&env, signature),
        };
        let check = |signature: &[u8; 64]| {
            client.try_check(
                &key,
                &policy,
                &recipient,
                destination(&env, &f),
                &attestation(signature),
                &now,
            )
        };

        prop_assert_eq!(check(&signed), Ok(Ok(f.valid_until)));

        let foreign = other.sign(&reference_message(&f)).to_bytes();
        prop_assert!(matches!(check(&foreign), Err(Err(_))), "another key passed");

        let mut flipped = signed;
        flipped[(bit / 8) % 64] ^= 1 << (bit % 8);
        prop_assert!(
            matches!(check(&flipped), Err(Err(_))),
            "a flipped bit passed"
        );

        // What a caller can change in a request: the recipient and the
        // destination. The rest comes from the mandate and the network,
        // and the properties above cover it through the message.
        for (name, changed) in flips(&f, bit) {
            if changed.network != f.network
                || changed.mandate != f.mandate
                || changed.country != f.country
                || changed.epoch != f.epoch
                || changed.valid_until != f.valid_until
            {
                continue;
            }
            let got = client.try_check(
                &key,
                &policy,
                &BytesN::from_array(&env, &changed.recipient),
                destination(&env, &changed),
                &attestation(&signed),
                &now,
            );
            prop_assert!(matches!(got, Err(Err(_))), "changing {} passed", name);
        }
        Ok(())
    }));
}

fn unhex(s: &str) -> Vec<u8> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn array32(b: &[u8]) -> [u8; 32] {
    b.try_into().unwrap()
}

/// Reads the layout back into its fields, so the encoders are checked against
/// bytes neither of them produced.
fn decode(bytes: &[u8]) -> Fields {
    let mut at = 0usize;
    let mut take = |n: usize| {
        let part = &bytes[at..at + n];
        at += n;
        part
    };
    let network = array32(take(32));
    let mandate_len = usize::from(take(1)[0]);
    let mandate = take(mandate_len);
    assert_eq!(mandate_len, 40, "a mandate is a contract");
    let mandate = array32(&mandate[8..]);
    let recipient = array32(take(32));
    let country = array32(take(32));
    let epoch = u64::from_be_bytes(take(8).try_into().unwrap());
    let valid_until = u64::from_be_bytes(take(8).try_into().unwrap());
    let dest_len = usize::from(take(1)[0]);
    let dest = take(dest_len);
    let dest = match dest_len {
        44 => Dest::Account(array32(&dest[12..])),
        40 => Dest::Contract(array32(&dest[8..])),
        n => panic!("an address of {n} bytes"),
    };
    let has_tag = take(1)[0];
    let tag = u64::from_be_bytes(take(8).try_into().unwrap());
    assert_eq!(at, bytes.len(), "bytes left over");
    Fields {
        network,
        mandate,
        recipient,
        country,
        epoch,
        valid_until,
        dest,
        tag: (has_tag != 0).then_some(tag),
    }
}

/// The contract's encoder and the reference agree byte for byte, on every
/// generated input and on every signed and checked message of the parity
/// fixture, which the reference signer built.
#[test]
fn attestation_encoding_matches_reference() {
    must_hold(runner().run(&fields(), |f| {
        let env = new_env();
        prop_assert_eq!(contract_message(&env, &f), reference_message(&f));
        Ok(())
    }));

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../spikes/attestation-parity/fixture.json");
    let fixture: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut messages = 0u32;
    for case in fixture["cases"].as_array().unwrap() {
        let name: String = case["name"].as_str().unwrap().into();
        for attestation in case["attestations"].as_array().unwrap() {
            for side in ["signed", "checked"] {
                let part = &attestation[side];
                let Some(raw) = part["fields"].as_str() else {
                    continue;
                };
                let raw = unhex(raw);
                let want = unhex(part["message"].as_str().unwrap());
                let f = decode(&raw);
                assert_eq!(reference_fields(&f), raw, "{name} {side} fields");
                assert_eq!(reference_message(&f), want, "{name} {side} reference");
                let env = new_env();
                assert_eq!(contract_message(&env, &f), want, "{name} {side} contract");
                messages += 1;
            }
        }
    }
    assert!(messages > 0, "the fixture held no messages");
    std::println!("attestation_encoding_matches_reference: {messages} fixture messages");
}
