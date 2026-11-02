// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs
//
// Reference signer and fixture builder for the attestation parity spike.
//
// It does three jobs, so the Soroban suite has something independent to agree
// with:
//   1. encodes the attested fields and builds the 47 byte message,
//   2. signs it with a test-only ed25519 key (and builds the tampered variants),
//   3. runs its own verifier over every case and refuses to write a fixture
//      whose declared outcome disagrees with it.
//
// Usage:
//   node generate.mjs <out.json>          write the fixture
//   node generate.mjs --check <out.json>  exit 1 if the file differs from a fresh build
//
// ed25519 signatures are deterministic and every input comes from a fixed
// label, so --check is a byte comparison.

import { readFileSync, writeFileSync } from "node:fs";
import { ed25519 } from "@noble/curves/ed25519.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { Account, Address, Keypair, MuxedAccount, StrKey } from "@stellar/stellar-sdk/base";

const PREFIX = "UJIRA/ATTEST/V2";
const MESSAGE_BYTES = 47;
const SIGNATURE_BYTES = 64;
const MAX_ATTESTATION_VALIDITY = 86400n;
const L = ed25519.Point.CURVE().n;
const U64_MAX = (1n << 64n) - 1n;
const I128_MAX = (1n << 127n) - 1n;

const ERR = { ATTESTATION_INVALID: 51, ATTESTATION_EXPIRED: 52, POLICY_CHANGED: 53 };
const HOST_TRAP = "HOST_TRAP";

const enc = new TextEncoder();
const hex = (b) => "0x" + Buffer.from(b).toString("hex");
const concat = (...parts) => {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let o = 0;
  for (const p of parts) {
    out.set(p, o);
    o += p.length;
  }
  return out;
};
const beBytes = (v, len) => {
  if (v < 0n || v >= 1n << BigInt(len * 8)) throw new Error(`value ${v} does not fit in ${len} bytes`);
  const out = new Uint8Array(len);
  for (let i = len - 1; i >= 0; i--) {
    out[i] = Number(v & 0xffn);
    v >>= 8n;
  }
  return out;
};
const leBytes = (v, len) => beBytes(v, len).reverse();
const fromLe = (b) => BigInt(hex(Uint8Array.from(b).reverse()));
const u8 = (v) => Uint8Array.of(v);

// Every key and address below comes from a public label, so nothing here can
// ever be a real key. Anyone can rebuild them from this file. The private keys
// are left out of the fixture on purpose: a consumer that needs one derives it
// from the label written next to the public key.
const LABEL_PREFIX = "UJIRA/TEST-ONLY/S18/";
const label = (name) => enc.encode(`${LABEL_PREFIX}${name}`);
const seedOf = (name) => sha256(label(name));

const KEY_LABELS = { attester: "attester-ed25519", other: "other-signer-ed25519" };
const SEEDS = { attester: seedOf(KEY_LABELS.attester), other: seedOf(KEY_LABELS.other) };
const PUB = { attester: ed25519.getPublicKey(SEEDS.attester), other: ed25519.getPublicKey(SEEDS.other) };

// Encoding of the identity point: y = 1, x = 0. It has small order, so a
// strict verifier refuses it as a key and as R.
const IDENTITY = (() => {
  const b = new Uint8Array(32);
  b[0] = 1;
  return b;
})();

const NETWORKS = {
  primary: "Test SDF Network ; September 2015",
  other: "Public Global Stellar Network ; September 2015",
};

const stellarAccount = (name) => Keypair.fromRawEd25519Seed(Buffer.from(seedOf(name))).publicKey();
const stellarContract = (name) => StrKey.encodeContract(Buffer.from(seedOf(name)));
const scAddressXdr = (strkey) => Uint8Array.from(Address.fromString(strkey).toScVal().toXDR());

const DEST = {
  primary: stellarAccount("dest-primary"),
  other: stellarAccount("dest-other"),
  contract: stellarContract("dest-forwarder"),
};
const MANDATE = {
  primary: stellarContract("mandate-primary"),
  other: stellarContract("mandate-other"),
};
const POLICY = { primary: sha256(label("policy-primary")), other: sha256(label("policy-other")) };
const REF = { primary: sha256(label("recipient-primary")), other: sha256(label("recipient-other")) };

// A destination with a tag is an M address over the same G. The fields use
// the base address bytes plus the tag, so G and M share one encoding and only
// the tag tells them apart.
function legOf(leg) {
  const hasTag = leg.tag !== null;
  const tag = hasTag ? leg.tag : 0n;
  const base = DEST[leg.dest];
  let destination = base;
  if (hasTag) {
    if (!base.startsWith("G")) throw new Error("only account destinations can carry a tag on Stellar");
    destination = new MuxedAccount(new Account(base, "0"), tag.toString()).accountId();
  }
  return {
    out: {
      destination,
      destination_xdr: hex(scAddressXdr(base)),
      has_tag: hasTag,
      tag: tag.toString(),
      amount: leg.amount.toString(),
    },
    destBytes: scAddressXdr(base),
    hasTag,
    tag,
  };
}

const lengthPrefixed = (b) => {
  if (b.length > 255) throw new Error("address longer than 255 bytes");
  return concat(u8(b.length), b);
};

// The attested fields, in order. Every field has a fixed width or a one byte
// length prefix, so two different field sequences never give the same bytes.
export function fields({ networkId, mandateBytes, recipientRef, policyHash, policyEpoch, validUntil, destBytes, hasTag, tag }) {
  if (networkId.length !== 32 || recipientRef.length !== 32 || policyHash.length !== 32) {
    throw new Error("fixed width field has the wrong length");
  }
  if (!hasTag && tag !== 0n) throw new Error("tag must be 0 when absent");
  return concat(
    networkId,
    lengthPrefixed(mandateBytes),
    recipientRef,
    policyHash,
    beBytes(policyEpoch, 8),
    beBytes(validUntil, 8),
    lengthPrefixed(destBytes),
    u8(hasTag ? 1 : 0),
    beBytes(tag, 8),
  );
}

export function message(fieldBytes) {
  const m = concat(enc.encode(PREFIX), sha256(fieldBytes));
  if (m.length !== MESSAGE_BYTES) throw new Error("message is not 47 bytes");
  return m;
}

const MUTATIONS = {
  truncate: (sig) => sig.slice(0, 63),
  extend: (sig) => concat(sig, u8(0)),
  empty: () => new Uint8Array(0),
  flip_r: (sig) => {
    const out = Uint8Array.from(sig);
    out[0] ^= 1;
    return out;
  },
  flip_s: (sig) => {
    const out = Uint8Array.from(sig);
    out[32] ^= 1;
    return out;
  },
  // S + L is the same scalar mod L in a non-canonical encoding. A verifier
  // that accepted it would make every signature malleable.
  s_plus_l: (sig) => concat(sig.slice(0, 32), leBytes(fromLe(sig.slice(32)) + L, 32)),
  zero: () => new Uint8Array(64),
  // R = identity, S = 0. With a small-order key this passes a lax,
  // cofactorless check for any message; with a real key it never verifies.
  identity_r_zero_s: () => concat(IDENTITY, new Uint8Array(32)),
};

// Strict ed25519 as the host does it: canonical S, and neither the key nor R
// may have small order. noble's non-ZIP-215 mode covers the encodings; the
// small-order checks are written out so they do not depend on it.
function strictVerify(sig, msg, pub) {
  if (sig.length !== SIGNATURE_BYTES || pub.length !== 32) return false;
  if (fromLe(sig.slice(32)) >= L) return false;
  let A;
  let R;
  try {
    A = ed25519.Point.fromBytes(pub);
    R = ed25519.Point.fromBytes(sig.slice(0, 32));
  } catch {
    return false;
  }
  if (A.isSmallOrder() || R.isSmallOrder()) return false;
  try {
    return ed25519.verify(sig, msg, pub, { zip215: false });
  } catch {
    return false;
  }
}

// The request-time rules, in the contract's order. A bad signature is not a
// code: the host traps, so the reference reports HOST_TRAP.
function referenceVerify(now, attesterPub, currentEpoch, pairs, legCount, attCount) {
  if (legCount !== attCount) return "ATTESTATION_INVALID";
  for (const p of pairs) {
    if (now >= p.validUntil) return "ATTESTATION_EXPIRED";
    if (p.validUntil - now > MAX_ATTESTATION_VALIDITY) return "ATTESTATION_INVALID";
    if (p.signature.length !== SIGNATURE_BYTES) return "ATTESTATION_INVALID";
    if (p.policyEpoch !== currentEpoch) return "POLICY_CHANGED";
    if (!strictVerify(p.signature, p.message, attesterPub)) return HOST_TRAP;
  }
  return "ok";
}

const NOW = 1767225600n; // 2026-01-01T00:00:00Z
const VU = NOW + 3600n;
const CTX = { network: "primary", mandate: "primary", policy: "primary", epoch: 0n, attester: "attester" };
const leg = (over = {}) => ({ dest: "primary", tag: null, amount: 500n, ...over });
const att = (over = {}) => ({ validUntil: VU, epoch: null, signer: "attester", signOver: {}, mutate: null, unprefixed: false, ...over });

// Each case: what the verifier sees (ctx, ref, legs, attestations) and, per
// attestation, what the signer actually signed (signOver) and any byte damage
// done afterwards (mutate). An attestation's policy_epoch defaults to the
// mandate's. Expectations are written by hand and checked against
// referenceVerify, so a mistake in either shows up.
const CASES = [
  { name: "valid_untagged", tamper: null, expect: "ok" },
  { name: "valid_tagged", tamper: null, expect: "ok", legs: [leg({ tag: 7n })] },
  { name: "valid_tag_zero", tamper: null, expect: "ok", legs: [leg({ tag: 0n })] },
  { name: "valid_tag_u64_max", tamper: null, expect: "ok", legs: [leg({ tag: U64_MAX })] },
  { name: "valid_contract_destination", tamper: null, expect: "ok", legs: [leg({ dest: "contract" })] },
  {
    name: "valid_two_legs_same_address_two_tags",
    tamper: null,
    expect: "ok",
    legs: [leg({ tag: 1n, amount: 300n }), leg({ tag: 2n, amount: 200n })],
    attestations: [att(), att()],
  },
  {
    name: "valid_window_upper_bound",
    tamper: null,
    expect: "ok",
    attestations: [att({ validUntil: NOW + MAX_ATTESTATION_VALIDITY })],
  },
  { name: "valid_last_second", tamper: null, expect: "ok", now: VU - 1n },
  { name: "valid_later_policy_epoch", tamper: null, expect: "ok", ctx: { epoch: 3n } },
  { name: "valid_policy_epoch_u64_max", tamper: null, expect: "ok", ctx: { epoch: U64_MAX } },
  { name: "amount_not_bound_one", tamper: "wrong_amount", expect: "ok", legs: [leg({ amount: 1n })] },
  { name: "amount_not_bound_i128_max", tamper: "wrong_amount", expect: "ok", legs: [leg({ amount: I128_MAX })] },
  { name: "replay_same_attestation", tamper: "replayed", expect: "ok", calls: 2 },
  { name: "wrong_signer", tamper: "wrong_signer", expect: HOST_TRAP, attestations: [att({ signer: "other" })] },
  {
    name: "wrong_chain",
    tamper: "wrong_chain",
    expect: HOST_TRAP,
    attestations: [att({ signOver: { network: "other" } })],
  },
  {
    name: "wrong_mandate",
    tamper: "wrong_mandate",
    expect: HOST_TRAP,
    attestations: [att({ signOver: { mandate: "other" } })],
  },
  {
    name: "wrong_destination",
    tamper: "wrong_destination",
    expect: HOST_TRAP,
    attestations: [att({ signOver: { dest: "other" } })],
  },
  {
    name: "wrong_tag_other_value",
    tamper: "wrong_tag",
    expect: HOST_TRAP,
    legs: [leg({ tag: 8n })],
    attestations: [att({ signOver: { tag: 7n } })],
  },
  {
    name: "wrong_tag_dropped",
    tamper: "wrong_tag",
    expect: HOST_TRAP,
    legs: [leg({ tag: null })],
    attestations: [att({ signOver: { tag: 7n } })],
  },
  {
    name: "wrong_tag_zero_versus_none",
    tamper: "wrong_tag",
    expect: HOST_TRAP,
    legs: [leg({ tag: 0n })],
    attestations: [att({ signOver: { tag: null } })],
  },
  {
    name: "wrong_policy",
    tamper: "wrong_policy",
    expect: HOST_TRAP,
    attestations: [att({ signOver: { policy: "other" } })],
  },
  { name: "wrong_ref", tamper: "wrong_ref", expect: HOST_TRAP, attestations: [att({ signOver: { ref: "other" } })] },
  {
    // The attester's own signature over SHA-256(fields) without the prefix,
    // which is what a verifier that dropped the version would accept.
    name: "unprefixed_message",
    tamper: "unprefixed_message",
    expect: HOST_TRAP,
    attestations: [att({ unprefixed: true })],
  },
  {
    name: "wrong_valid_until",
    tamper: "wrong_valid_until",
    expect: HOST_TRAP,
    attestations: [att({ validUntil: VU + 60n, signOver: { validUntil: VU } })],
  },
  {
    name: "wrong_policy_epoch_signed",
    tamper: "wrong_epoch",
    expect: HOST_TRAP,
    ctx: { epoch: 1n },
    attestations: [att({ signOver: { epoch: 0n } })],
  },
  { name: "stale_policy_epoch", tamper: "stale_epoch", expect: "POLICY_CHANGED", ctx: { epoch: 1n }, attestations: [att({ epoch: 0n })] },
  { name: "future_policy_epoch", tamper: "stale_epoch", expect: "POLICY_CHANGED", ctx: { epoch: 1n }, attestations: [att({ epoch: 2n })] },
  {
    name: "stale_policy_epoch_wins_over_bad_signature",
    tamper: "stale_epoch",
    expect: "POLICY_CHANGED",
    ctx: { epoch: 1n },
    attestations: [att({ epoch: 0n, signer: "other" })],
  },
  { name: "signature_63_bytes", tamper: "truncated", expect: "ATTESTATION_INVALID", attestations: [att({ mutate: "truncate" })] },
  { name: "signature_65_bytes", tamper: "malformed", expect: "ATTESTATION_INVALID", attestations: [att({ mutate: "extend" })] },
  { name: "empty_signature", tamper: "malformed", expect: "ATTESTATION_INVALID", attestations: [att({ mutate: "empty" })] },
  {
    name: "short_signature_wins_over_stale_epoch",
    tamper: "truncated",
    expect: "ATTESTATION_INVALID",
    ctx: { epoch: 1n },
    attestations: [att({ epoch: 0n, mutate: "truncate" })],
  },
  { name: "flipped_r_bit", tamper: "bad_signature", expect: HOST_TRAP, attestations: [att({ mutate: "flip_r" })] },
  { name: "flipped_s_bit", tamper: "bad_signature", expect: HOST_TRAP, attestations: [att({ mutate: "flip_s" })] },
  { name: "non_canonical_s", tamper: "bad_signature", expect: HOST_TRAP, attestations: [att({ mutate: "s_plus_l" })] },
  { name: "zero_signature", tamper: "bad_signature", expect: HOST_TRAP, attestations: [att({ mutate: "zero" })] },
  {
    name: "identity_r_zero_s",
    tamper: "bad_signature",
    expect: HOST_TRAP,
    attestations: [att({ mutate: "identity_r_zero_s" })],
  },
  {
    name: "small_order_attester_key",
    tamper: "bad_signature",
    expect: HOST_TRAP,
    ctx: { attester: "identity" },
    attestations: [att({ mutate: "identity_r_zero_s" })],
  },
  { name: "expired_at_valid_until", tamper: "expired", expect: "ATTESTATION_EXPIRED", now: VU },
  { name: "expired_after_valid_until", tamper: "expired", expect: "ATTESTATION_EXPIRED", now: VU + 86400n },
  {
    name: "expired_wins_over_stale_epoch",
    tamper: "expired",
    expect: "ATTESTATION_EXPIRED",
    now: VU,
    ctx: { epoch: 1n },
    attestations: [att({ epoch: 0n })],
  },
  {
    name: "window_too_long",
    tamper: "window",
    expect: "ATTESTATION_INVALID",
    attestations: [att({ validUntil: NOW + MAX_ATTESTATION_VALIDITY + 1n })],
  },
  { name: "missing_attestation", tamper: "count", expect: "ATTESTATION_INVALID", attestations: [] },
  { name: "extra_attestation", tamper: "count", expect: "ATTESTATION_INVALID", attestations: [att(), att()] },
  {
    name: "attestations_swapped_between_legs",
    tamper: "wrong_tag",
    expect: HOST_TRAP,
    legs: [leg({ tag: 1n, amount: 300n }), leg({ tag: 2n, amount: 200n })],
    attestations: [att({ signOver: { tag: 2n } }), att({ signOver: { tag: 1n } })],
  },
  {
    name: "second_leg_wrong_destination",
    tamper: "wrong_destination",
    expect: HOST_TRAP,
    legs: [leg({ amount: 300n }), leg({ dest: "other", amount: 200n })],
    attestations: [att(), att({ signOver: { dest: "primary" } })],
  },
  {
    name: "second_leg_stale_policy_epoch",
    tamper: "stale_epoch",
    expect: "POLICY_CHANGED",
    ctx: { epoch: 1n },
    legs: [leg({ amount: 300n }), leg({ dest: "other", amount: 200n })],
    attestations: [att(), att({ epoch: 0n })],
  },
  {
    name: "first_leg_expired_wins_over_second_bad_signature",
    tamper: "expired",
    expect: "ATTESTATION_EXPIRED",
    now: VU,
    legs: [leg({ amount: 300n }), leg({ dest: "other", amount: 200n })],
    attestations: [att(), att({ validUntil: VU + 60n, signer: "other" })],
  },
  {
    name: "first_leg_bad_signature_wins_over_second_expired",
    tamper: "wrong_signer",
    expect: HOST_TRAP,
    now: VU,
    legs: [leg({ amount: 300n }), leg({ dest: "other", amount: 200n })],
    attestations: [att({ validUntil: VU + 60n, signer: "other" }), att()],
  },
];

const attesterKey = (name) => (name === "identity" ? IDENTITY : PUB[name]);

function contextOf(ctx) {
  const passphrase = NETWORKS[ctx.network];
  return {
    passphrase,
    networkId: sha256(enc.encode(passphrase)),
    mandate: MANDATE[ctx.mandate],
    mandateBytes: scAddressXdr(MANDATE[ctx.mandate]),
  };
}

function fieldsFor(context, { ref, policy, epoch, validUntil, l }) {
  return fields({
    networkId: context.networkId,
    mandateBytes: context.mandateBytes,
    recipientRef: REF[ref],
    policyHash: POLICY[policy],
    policyEpoch: epoch,
    validUntil,
    destBytes: l.destBytes,
    hasTag: l.hasTag,
    tag: l.tag,
  });
}

const encoded = (f) => {
  const m = message(f);
  return { fields: hex(f), fields_sha256: hex(sha256(f)), message: hex(m) };
};

function buildCase(c) {
  const now = c.now ?? NOW;
  const ctx = { ...CTX, ...(c.ctx ?? {}) };
  const ref = c.ref ?? "primary";
  const legs = c.legs ?? [leg()];
  const atts = c.attestations ?? legs.map(() => att());
  const attesterPub = attesterKey(ctx.attester);
  const presented = contextOf(ctx);
  const caseLegs = legs.map(legOf);

  const pairs = [];
  const outAtts = atts.map((a, i) => {
    const epoch = a.epoch ?? ctx.epoch;
    const legSpec = legs[Math.min(i, legs.length - 1)];
    const so = a.signOver;
    const signedFields = fieldsFor(contextOf({ ...ctx, network: so.network ?? ctx.network, mandate: so.mandate ?? ctx.mandate }), {
      ref: so.ref ?? ref,
      policy: so.policy ?? ctx.policy,
      epoch: so.epoch ?? epoch,
      validUntil: so.validUntil ?? a.validUntil,
      l: legOf({ ...legSpec, dest: so.dest ?? legSpec.dest, tag: "tag" in so ? so.tag : legSpec.tag }),
    });
    const signed = encoded(signedFields);
    const signedMessage = message(signedFields);
    const sig = ed25519.sign(a.unprefixed ? sha256(signedFields) : signedMessage, SEEDS[a.signer]);
    const signature = a.mutate === null ? sig : MUTATIONS[a.mutate](sig);
    // What the contract will build: its own network and address, the
    // presented leg at the same index, the presented valid_until and epoch.
    // Null when there is no leg for this attestation.
    const l = caseLegs[i];
    let checked = null;
    if (l) {
      const f = fieldsFor(presented, { ref, policy: ctx.policy, epoch, validUntil: a.validUntil, l });
      checked = encoded(f);
      pairs.push({ validUntil: a.validUntil, policyEpoch: epoch, signature, message: message(f) });
    }
    return {
      valid_until: a.validUntil.toString(),
      policy_epoch: epoch.toString(),
      signature: hex(signature),
      signed,
      checked,
    };
  });

  const outcome = referenceVerify(now, attesterPub, ctx.epoch, pairs, legs.length, atts.length);
  if (outcome !== c.expect) throw new Error(`${c.name}: expected ${c.expect}, reference says ${outcome}`);
  return {
    name: c.name,
    tamper: c.tamper,
    expect: c.expect,
    expect_code: c.expect === "ok" ? 0 : (ERR[c.expect] ?? null),
    calls: c.calls ?? 1,
    now: now.toString(),
    network_passphrase: presented.passphrase,
    network_id: hex(presented.networkId),
    mandate: presented.mandate,
    mandate_xdr: hex(presented.mandateBytes),
    attester_public_key: hex(attesterPub),
    recipient_ref: hex(REF[ref]),
    country_policy_hash: hex(POLICY[ctx.policy]),
    policy_epoch: ctx.epoch.toString(),
    legs: caseLegs.map((x) => x.out),
    attestations: outAtts,
  };
}

function build() {
  return {
    warning:
      "TEST ONLY. Every key in this file is derived from a public label and must never hold funds, sign anything real or be configured as an attester outside tests.",
    format_version: 2,
    message_prefix: PREFIX,
    message_bytes: MESSAGE_BYTES,
    signature_bytes: SIGNATURE_BYTES,
    max_attestation_validity: MAX_ATTESTATION_VALIDITY.toString(),
    error_codes: ERR,
    fields_layout: [
      "network_id: 32 bytes, SHA-256 of the network passphrase",
      "mandate: u8 length, then Address::to_xdr of the mandate contract",
      "recipient_ref: 32 bytes",
      "country_policy_hash: 32 bytes",
      "policy_epoch: u64 big-endian",
      "valid_until: u64 big-endian",
      "destination: u8 length, then Address::to_xdr of the base G or C address",
      "has_tag: u8, 0 or 1",
      "tag: u64 big-endian, 0 when absent",
    ],
    message_layout: "message_prefix (15 ASCII bytes) || SHA-256(fields) (32 bytes)",
    keys: {
      attester: {
        seed: `sha256("${LABEL_PREFIX}${KEY_LABELS.attester}")`,
        public_key: hex(PUB.attester),
      },
      other_signer: {
        seed: `sha256("${LABEL_PREFIX}${KEY_LABELS.other}")`,
        public_key: hex(PUB.other),
      },
    },
    cases: CASES.map(buildCase),
  };
}

const args = process.argv.slice(2);
const check = args[0] === "--check";
const out = check ? args[1] : args[0];
if (!out) {
  console.error("usage: node generate.mjs [--check] <fixture.json>");
  process.exit(2);
}
const text = JSON.stringify(build(), null, 2) + "\n";
if (check) {
  const current = readFileSync(out, "utf8");
  if (current !== text) {
    console.error(`${out} is out of date; run node generate.mjs ${out}`);
    process.exit(1);
  }
  console.log(`${out} matches a fresh build`);
} else {
  writeFileSync(out, text);
  console.log(`wrote ${out}`);
}
