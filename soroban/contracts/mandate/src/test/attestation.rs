// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! Attested requests: the per-leg checks that return a code, in their order,
//! the signed message byte for byte, and the trap a bad signature causes.
//! Valid signatures over the mandate's own message are covered by the
//! shared vectors, which sign with the real test keys.

extern crate std;

use super::{code, dest, mandate_wasm, Fx, BOB_WALLET, DAY, NOW};
use crate::attest;
use crate::types::{Attestation, Leg, PayoutRequest};
use crate::{MandateConfig, MandateError, UjiraMandateClient};
use mandate_config::{MODE_ATTESTED, MODE_BOTH};
use soroban_sdk::testutils::Ledger as _;
use soroban_sdk::{Address, Bytes, BytesN, Vec};

/// The attester's key in these tests: 32 bytes no role account has as its
/// key.
const ATTESTER: [u8; 32] = [0x42; 32];

fn attested_config(fx: &Fx, mode: u32) -> MandateConfig {
    let mut config = fx.config();
    config.recipient_mode = mode;
    config.attester = Some(Bytes::from_array(&fx.env, &ATTESTER));
    config
}

fn attested(fx: &Fx, mode: u32) -> UjiraMandateClient<'_> {
    let id = fx.create(&attested_config(fx, mode)).unwrap();
    fx.mint(&id, 10_000);
    UjiraMandateClient::new(&fx.env, &id)
}

fn att(fx: &Fx, valid_until: u64, policy_epoch: u64, sig_len: usize) -> Attestation {
    let sig = std::vec![7u8; sig_len];
    Attestation {
        valid_until,
        policy_epoch,
        signature: Bytes::from_slice(&fx.env, &sig),
    }
}

fn with(fx: &Fx, id: u8, atts: &[Attestation]) -> PayoutRequest {
    let mut req = fx.req(id, 100);
    req.attestations = Vec::new(&fx.env);
    for a in atts {
        req.attestations.push_back(a.clone());
    }
    req
}

fn refused(fx: &Fx, m: &UjiraMandateClient, req: &PayoutRequest) -> MandateError {
    code(m.try_request_payout(&fx.agent, req))
}

#[test]
fn attestation_63_bytes_returns_51() {
    let fx = Fx::new();
    let m = attested(&fx, MODE_ATTESTED);
    let req = with(&fx, 1, &[att(&fx, NOW + 3_600, 0, 63)]);
    assert_eq!(refused(&fx, &m, &req), MandateError::AttestationInvalid);
    assert_eq!(MandateError::AttestationInvalid as u32, 51);
}

#[test]
fn attestation_65_bytes_returns_51() {
    let fx = Fx::new();
    let m = attested(&fx, MODE_ATTESTED);
    let req = with(&fx, 1, &[att(&fx, NOW + 3_600, 0, 65)]);
    assert_eq!(refused(&fx, &m, &req), MandateError::AttestationInvalid);
}

#[test]
fn attestation_empty_and_extended_signatures_return_51() {
    let fx = Fx::new();
    let m = attested(&fx, MODE_ATTESTED);
    for len in [0, 1, 32, 128, 1_000] {
        let req = with(&fx, 1, &[att(&fx, NOW + 3_600, 0, len)]);
        assert_eq!(
            refused(&fx, &m, &req),
            MandateError::AttestationInvalid,
            "{len} bytes"
        );
    }
}

#[test]
fn attestation_count_must_match_the_legs() {
    let fx = Fx::new();
    let m = attested(&fx, MODE_ATTESTED);
    let a = att(&fx, NOW + 3_600, 0, 64);
    assert_eq!(
        refused(&fx, &m, &with(&fx, 1, &[])),
        MandateError::AttestationInvalid
    );
    assert_eq!(
        refused(&fx, &m, &with(&fx, 1, &[a.clone(), a])),
        MandateError::AttestationInvalid
    );
}

#[test]
fn attestation_checks_in_order_before_the_signature() {
    let fx = Fx::new();
    let m = attested(&fx, MODE_ATTESTED);
    // Expired at valid_until itself, whatever else is wrong with it.
    let req = with(&fx, 1, &[att(&fx, NOW, 5, 3)]);
    assert_eq!(refused(&fx, &m, &req), MandateError::AttestationExpired);
    let req = with(&fx, 1, &[att(&fx, NOW - 1, 5, 3)]);
    assert_eq!(refused(&fx, &m, &req), MandateError::AttestationExpired);
    // A window one second too long, before the length and the epoch.
    let req = with(&fx, 1, &[att(&fx, NOW + DAY + 1, 5, 3)]);
    assert_eq!(refused(&fx, &m, &req), MandateError::AttestationInvalid);
    // The longest window passes on to the length.
    let req = with(&fx, 1, &[att(&fx, NOW + DAY, 5, 63)]);
    assert_eq!(refused(&fx, &m, &req), MandateError::AttestationInvalid);
    // The length before the epoch.
    let req = with(&fx, 1, &[att(&fx, u64::MAX, 5, 3)]);
    assert_eq!(refused(&fx, &m, &req), MandateError::AttestationInvalid);
    // A well-formed attestation from another policy epoch gets its own code
    // and never reaches the signature.
    let req = with(&fx, 1, &[att(&fx, NOW + DAY, 1, 64)]);
    assert_eq!(refused(&fx, &m, &req), MandateError::PolicyChanged);
}

#[test]
fn attestation_first_failing_leg_decides() {
    let fx = Fx::new();
    let m = attested(&fx, MODE_ATTESTED);
    let mut req = with(
        &fx,
        1,
        &[att(&fx, NOW, 0, 64), att(&fx, NOW + 3_600, 0, 10)],
    );
    req.legs = Vec::from_array(
        &fx.env,
        [
            Leg {
                destination: fx.bob_wallet(),
                amount: 60,
            },
            Leg {
                destination: dest(&fx.env, 51, None),
                amount: 40,
            },
        ],
    );
    assert_eq!(refused(&fx, &m, &req), MandateError::AttestationExpired);
    // Swapped, the second leg's problem is the first one met.
    req.attestations = Vec::from_array(
        &fx.env,
        [att(&fx, NOW + 3_600, 0, 10), att(&fx, NOW, 0, 64)],
    );
    assert_eq!(refused(&fx, &m, &req), MandateError::AttestationInvalid);
}

#[test]
fn both_mode_checks_allowlist_then_attestation_per_leg() {
    let fx = Fx::new();
    let m = attested(&fx, MODE_BOTH);
    // Not on the allowlist comes before an expired attestation.
    let req = with(&fx, 1, &[att(&fx, NOW, 0, 64)]);
    assert_eq!(refused(&fx, &m, &req), MandateError::RecipientNotAllowed);
    fx.allow(&m.address, &fx.bob(), &fx.bob_wallet());
    assert_eq!(refused(&fx, &m, &req), MandateError::AttestationExpired);
    // The first leg is checked in full before the second: its expired
    // attestation wins over the second leg's missing allowlist entry.
    let mut two = with(&fx, 2, &[att(&fx, NOW, 0, 64), att(&fx, NOW + 60, 0, 64)]);
    two.legs = Vec::from_array(
        &fx.env,
        [
            Leg {
                destination: fx.bob_wallet(),
                amount: 60,
            },
            Leg {
                destination: dest(&fx.env, 51, None),
                amount: 40,
            },
        ],
    );
    assert_eq!(refused(&fx, &m, &two), MandateError::AttestationExpired);
}

#[test]
fn bad_signature_traps_and_aborts_the_whole_batch() {
    let fx = Fx::new();
    let m = attested(&fx, MODE_ATTESTED);
    let before = m.state();
    // A well-formed 64-byte signature that the attester never made.
    let bad = with(&fx, 1, &[att(&fx, NOW + 3_600, 0, 64)]);
    let r = m.try_request_payout(&fx.agent, &bad);
    assert!(matches!(r, Err(Err(_))), "expected a host trap, got {r:?}");
    // In a batch, an earlier item that failed with a code is undone with
    // the rest: no result, no event, nothing written.
    let mut late = with(&fx, 2, &[att(&fx, NOW + 3_600, 0, 64)]);
    late.deadline = NOW;
    let reqs = Vec::from_array(&fx.env, [late, bad]);
    let r = m.try_request_payouts(&fx.agent, &reqs);
    assert!(matches!(r, Err(Err(_))), "expected a host trap, got {r:?}");
    assert!(fx.events(&m.address).is_empty());
    assert_eq!(m.state(), before);
    assert_eq!(m.payout(&fx.b32(1)), None);
}

#[test]
fn attested_mode_ignores_the_allowlist() {
    let fx = Fx::new();
    let m = attested(&fx, MODE_ATTESTED);
    // Nothing is allowed, and the request still gets as far as the
    // attestation.
    let req = with(&fx, 1, &[att(&fx, NOW, 0, 64)]);
    assert_eq!(refused(&fx, &m, &req), MandateError::AttestationExpired);
}

fn hex(s: &str) -> std::vec::Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn b32(fx: &Fx, s: &str) -> BytesN<32> {
    BytesN::from_array(&fx.env, &hex(s).try_into().unwrap())
}

/// The worked example of the attestation encoding, rebuilt by the mandate
/// at the example's own address and network, and checked against the
/// example's signature by the host.
#[test]
fn attestation_message_matches_the_worked_example() {
    let fx = Fx::new();
    let network: [u8; 32] = hex("cee0302d59844d32bdca915c8203dd44b33fbb7edc19051ea37abedf28ecd472")
        .try_into()
        .unwrap();
    fx.env.ledger().set_network_id(network);
    let at = Address::from_str(
        &fx.env,
        "CDUKSTCG3XKTPTHMNRJZIW23J3T5DKHASDPQ3CIRKQ4YJH3PDOFFULZA",
    );
    let config = fx.config();
    let m = fx.env.register_at(
        &at,
        mandate_wasm(),
        (fx.factory.clone(), fx.fee_address.clone(), config),
    );
    let destination = soroban_sdk::MuxedAddress::from(Address::from_str(
        &fx.env,
        "GDTYAUMMX45E2SU2Q7JRJHCXDIYTEUAXVBHZ7DVCVXZGMAKMWFL7SOY4",
    ));
    let recipient = b32(
        &fx,
        "187204dacf1b6ae9830406f0405b3710d69d86df034cf3fe3753e7b460c8e97d",
    );
    let policy = b32(
        &fx,
        "343fb1d75ee948735be1a836a29a40ecd0612986ab9bd1183f8271ccca830415",
    );
    // Each field on its own line: network, mandate, recipient, country
    // policy, policy epoch, valid until, destination, tag.
    let fields = concat!(
        "cee0302d59844d32bdca915c8203dd44b33fbb7edc19051ea37abedf28ecd472",
        "28",
        "0000001200000001e8a94c46ddd537ccec6c53945b5b4ee7d1a8e090df0d89115439849f6f1b8a5a",
        "187204dacf1b6ae9830406f0405b3710d69d86df034cf3fe3753e7b460c8e97d",
        "343fb1d75ee948735be1a836a29a40ecd0612986ab9bd1183f8271ccca830415",
        "0000000000000000",
        "000000006955c710",
        "2c",
        "000000120000000000000000e780518cbf3a4d4a9a87d3149c571a31325017a84f9f8ea2adf266014cb157f9",
        "00",
        "0000000000000000",
    );
    let message = concat!(
        "554a4952412f4154544553542f5632",
        "6786dd0154eaa55fca26d77da8952c91df2fcc0927e381771667f4c37a7f115e",
    );
    let key = b32(
        &fx,
        "421adb50c10fe73fb1fc0d4211a16911a188bb001eaf0391b479316026141d48",
    );
    let sig: [u8; 64] = hex(concat!(
        "043dcca5e9a6dfd41320f9ecf7b4778bf00ff706ac26d865f4c1ff6366a49e04",
        "97e63ef43e2a0e6404218231d10d11aa030f954d9eb799ff92f0328e1cae2208",
    ))
    .try_into()
    .unwrap();
    fx.env.as_contract(&m, || {
        let got =
            attest::fields(&fx.env, &recipient, &policy, 0, &destination, 1_767_229_200).unwrap();
        assert_eq!(got, Bytes::from_slice(&fx.env, &hex(fields)));
        assert_eq!(got.len(), 207);
        let msg =
            attest::message(&fx.env, &recipient, &policy, 0, &destination, 1_767_229_200).unwrap();
        assert_eq!(msg, Bytes::from_slice(&fx.env, &hex(message)));
        assert_eq!(msg.len(), 47);
        // Traps if the signature does not verify.
        fx.env
            .crypto()
            .ed25519_verify(&key, &msg, &BytesN::from_array(&fx.env, &sig));
        // A tag is bound with its presence: tag 0 is not no tag.
        let tagged = dest(&fx.env, BOB_WALLET, Some(0));
        let untagged = dest(&fx.env, BOB_WALLET, None);
        assert_ne!(
            attest::message(&fx.env, &recipient, &policy, 0, &tagged, 1).unwrap(),
            attest::message(&fx.env, &recipient, &policy, 0, &untagged, 1).unwrap()
        );
    });
}
