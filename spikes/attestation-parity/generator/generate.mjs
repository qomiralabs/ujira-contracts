// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs
//
// Reference signer and fixture builder for the attestation parity spike.
//
// It does three jobs, so the Soroban and Celo suites have something independent
// to agree with:
//   1. encodes the digest inputs for each chain and hashes them,
//   2. signs with a test-only key (and builds the tampered variants),
//   3. runs its own verifier over every case and refuses to write a fixture
//      whose declared outcome disagrees with it.
//
// Usage:
//   node generate.mjs <out.json>          write the fixture
//   node generate.mjs --check <out.json>  exit 1 if the file differs from a fresh build
//
// The output is deterministic (RFC 6979 nonces, fixed labels), so --check is a
// byte comparison.

import { readFileSync, writeFileSync } from "node:fs";
import { secp256k1 } from "@noble/curves/secp256k1.js";
import { keccak_256 } from "@noble/hashes/sha3.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { Account, Address, Keypair, MuxedAccount, StrKey } from "@stellar/stellar-base";

const DOMAIN = "UJIRA/ATTEST/V1";
const MAX_ATTESTATION_VALIDITY = 86400n;
const N = secp256k1.Point.CURVE().n;
const HALF_N = N / 2n;
const U64_MAX = (1n << 64n) - 1n;
const I128_MAX = (1n << 127n) - 1n;

const ERR = { ATTESTATION_INVALID: 51, ATTESTATION_EXPIRED: 52 };

const enc = new TextEncoder();
const hex = (b) => "0x" + Buffer.from(b).toString("hex");
const unhex = (h) => Uint8Array.from(Buffer.from(h.replace(/^0x/, ""), "hex"));
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
const toBig = (b) => BigInt(hex(b));
const u8 = (v) => Uint8Array.of(v);

// Every key and address below comes from a public label, so nothing here can
// ever be a real key. Anyone can rebuild them from this file.
const label = (name) => enc.encode(`UJIRA/TEST-ONLY/S18/${name}`);
const testKey = (name) => {
  const k = toBig(keccak_256(label(name)));
  if (k === 0n || k >= N) throw new Error(`label ${name} gives an invalid key`);
  return beBytes(k, 32);
};

const KEYS = {
  attester: testKey("attester"),
  other: testKey("other-signer"),
};
const pub = (sk) => secp256k1.getPublicKey(sk, false);
const evmAddress = (pk65) => hex(keccak_256(pk65.slice(1)).slice(12));

// Chain encodings. The contract side computes the same bytes from its own
// environment: network id and contract address on Soroban, chainid and
// address(this) on Celo.
const SOROBAN_NETWORKS = {
  primary: "Test SDF Network ; September 2015",
  other: "Public Global Stellar Network ; September 2015",
};
const CELO_CHAINS = { primary: 11142220n, other: 42220n };

const stellarAccount = (name) => Keypair.fromRawEd25519Seed(Buffer.from(sha256(label(name)))).publicKey();
const stellarContract = (name) => StrKey.encodeContract(Buffer.from(sha256(label(name))));
const scAddressXdr = (strkey) => Uint8Array.from(Address.fromString(strkey).toScVal().toXDR());
const celoAddress = (name) => hex(keccak_256(label(name)).slice(12));

const DEST = {
  primary: { soroban: stellarAccount("dest-primary"), celo: celoAddress("dest-primary") },
  other: { soroban: stellarAccount("dest-other"), celo: celoAddress("dest-other") },
  contract: { soroban: stellarContract("dest-forwarder"), celo: celoAddress("dest-forwarder") },
};
const MANDATE = {
  primary: { soroban: stellarContract("mandate-primary"), celo: celoAddress("mandate-primary") },
  other: { soroban: stellarContract("mandate-other"), celo: celoAddress("mandate-other") },
};
const POLICY = { primary: sha256(label("policy-primary")), other: sha256(label("policy-other")) };
const REF = { primary: sha256(label("recipient-primary")), other: sha256(label("recipient-other")) };

function chainContext(chain, ctx) {
  if (chain === "soroban") {
    const passphrase = SOROBAN_NETWORKS[ctx.chain];
    const mandate = MANDATE[ctx.mandate].soroban;
    return {
      fields: { network_passphrase: passphrase, chain_tag: hex(sha256(enc.encode(passphrase))), mandate },
      chainTag: sha256(enc.encode(passphrase)),
      mandateBytes: scAddressXdr(mandate),
      extra: { mandate_xdr: hex(scAddressXdr(mandate)) },
    };
  }
  const id = CELO_CHAINS[ctx.chain];
  const mandate = MANDATE[ctx.mandate].celo;
  return {
    fields: { chain_id: id.toString(), chain_tag: hex(beBytes(id, 32)), mandate },
    chainTag: beBytes(id, 32),
    mandateBytes: unhex(mandate),
    extra: {},
  };
}

// A Soroban destination with a tag is an M address over the same G; the digest
// uses the base address bytes plus the tag, so G and M share one encoding.
function chainLeg(chain, leg) {
  const hasTag = leg.tag !== null;
  const tag = hasTag ? leg.tag : 0n;
  if (chain === "soroban") {
    const base = DEST[leg.dest].soroban;
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
  const destination = DEST[leg.dest].celo;
  return {
    out: { destination, has_tag: hasTag, tag: tag.toString(), amount: leg.amount.toString() },
    destBytes: unhex(destination),
    hasTag,
    tag,
  };
}

export function digest({ chainTag, recipientRef, policyHash, validUntil, mandateBytes, destBytes, hasTag, tag }) {
  if (chainTag.length !== 32 || recipientRef.length !== 32 || policyHash.length !== 32) {
    throw new Error("fixed width field has the wrong length");
  }
  if (mandateBytes.length > 255 || destBytes.length > 255) throw new Error("address longer than 255 bytes");
  if (!hasTag && tag !== 0n) throw new Error("tag must be 0 when absent");
  return keccak_256(
    concat(
      enc.encode(DOMAIN),
      chainTag,
      recipientRef,
      policyHash,
      beBytes(validUntil, 8),
      u8(mandateBytes.length),
      mandateBytes,
      u8(destBytes.length),
      destBytes,
      u8(hasTag ? 1 : 0),
      beBytes(tag, 8),
    ),
  );
}

// r || s || v with v in {27, 28}, low s, the layout both contracts read.
function sign(d, sk) {
  const rec = secp256k1.sign(d, sk, { prehash: false, lowS: true, format: "recovered" });
  return concat(rec.slice(1, 65), u8(27 + rec[0]));
}

const firstXWithoutPoint = (() => {
  const { Fp } = secp256k1.Point;
  for (let x = 1n; ; x++) {
    try {
      Fp.sqrt(Fp.add(Fp.mul(Fp.sqr(x), x), 7n));
    } catch {
      return x;
    }
  }
})();

const MUTATIONS = {
  high_s: (sig) => {
    const s = toBig(sig.slice(32, 64));
    return concat(sig.slice(0, 32), beBytes(N - s, 32), u8(sig[64] === 27 ? 28 : 27));
  },
  v: (value) => (sig) => concat(sig.slice(0, 64), u8(value)),
  truncate: (sig) => sig.slice(0, 64),
  extend: (sig) => concat(sig, u8(0)),
  empty: () => new Uint8Array(0),
  r_zero: (sig) => concat(new Uint8Array(32), sig.slice(32)),
  s_zero: (sig) => concat(sig.slice(0, 32), new Uint8Array(32), sig.slice(64)),
  r_order: (sig) => concat(beBytes(N, 32), sig.slice(32)),
  s_half_order: (sig) => concat(sig.slice(0, 32), beBytes(HALF_N, 32), sig.slice(64)),
  s_half_order_plus_one: (sig) => concat(sig.slice(0, 32), beBytes(HALF_N + 1n, 32), sig.slice(64)),
  r_not_on_curve: (sig) => concat(beBytes(firstXWithoutPoint, 32), sig.slice(32)),
  // With s = 1 and R = e * G, recovery gives r^-1 * (s * R - e * G), the point
  // at infinity. Only the submitter can build this; no honest signer does.
  recovers_to_infinity: (_sig, presentedDigest) => {
    const e = toBig(presentedDigest) % N;
    const R = secp256k1.Point.BASE.multiply(e);
    const { x, y } = R.toAffine();
    if (x >= N) throw new Error("R.x not below n, pick another case");
    return concat(beBytes(x, 32), beBytes(1n, 32), u8(27 + Number(y & 1n)));
  },
};

// The rules both contracts implement, in their order. Returns
// { code, hostAbort } where hostAbort means Soroban's recover host function
// traps instead of letting the contract return the code.
function referenceVerify(now, attesterPub, pairs, legCount, attCount) {
  if (legCount !== attCount) return { code: "ATTESTATION_INVALID", hostAbort: false };
  for (const p of pairs) {
    if (now >= p.validUntil) return { code: "ATTESTATION_EXPIRED", hostAbort: false };
    if (p.validUntil - now > MAX_ATTESTATION_VALIDITY) return { code: "ATTESTATION_INVALID", hostAbort: false };
    const sig = p.signature;
    if (sig.length !== 65) return { code: "ATTESTATION_INVALID", hostAbort: false };
    const v = sig[64];
    if (v !== 27 && v !== 28) return { code: "ATTESTATION_INVALID", hostAbort: false };
    const r = toBig(sig.slice(0, 32));
    const s = toBig(sig.slice(32, 64));
    if (r === 0n || r >= N || s === 0n || s > HALF_N) return { code: "ATTESTATION_INVALID", hostAbort: false };
    let recovered;
    try {
      recovered = secp256k1.Signature.fromBytes(sig.slice(0, 64), "compact")
        .addRecoveryBit(v - 27)
        .recoverPublicKey(p.digest)
        .toBytes(false);
    } catch {
      return { code: "ATTESTATION_INVALID", hostAbort: true };
    }
    if (hex(recovered) !== hex(attesterPub)) return { code: "ATTESTATION_INVALID", hostAbort: false };
  }
  return { code: "ok", hostAbort: false };
}

const NOW = 1767225600n; // 2026-01-01T00:00:00Z
const VU = NOW + 3600n;
const CTX = { chain: "primary", mandate: "primary", policy: "primary" };
const leg = (over = {}) => ({ dest: "primary", tag: null, amount: 500n, ...over });
const att = (over = {}) => ({ validUntil: VU, signer: "attester", signOver: {}, mutate: null, ...over });

// Each case: what the verifier sees (ctx, ref, legs, attestations) and, per
// attestation, what the signer actually signed (signOver) and any byte damage
// done afterwards (mutate). Expectations are written by hand and checked
// against referenceVerify, so a mistake in either shows up.
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
  { name: "amount_not_bound_one", tamper: "wrong_amount", expect: "ok", legs: [leg({ amount: 1n })] },
  { name: "amount_not_bound_i128_max", tamper: "wrong_amount", expect: "ok", legs: [leg({ amount: I128_MAX })] },
  { name: "replay_same_attestation", tamper: "replayed", expect: "ok", calls: 2 },
  { name: "wrong_signer", tamper: "wrong_signer", expect: "ATTESTATION_INVALID", attestations: [att({ signer: "other" })] },
  { name: "high_s", tamper: "high_s", expect: "ATTESTATION_INVALID", attestations: [att({ mutate: "high_s" })] },
  ...[0, 1, 26, 29, 30, 255].map((v) => ({
    name: `bad_v_${v}`,
    tamper: "bad_v",
    expect: "ATTESTATION_INVALID",
    attestations: [att({ mutate: ["v", v] })],
  })),
  {
    name: "wrong_chain",
    tamper: "wrong_chain",
    expect: "ATTESTATION_INVALID",
    attestations: [att({ signOver: { chain: "other" } })],
  },
  {
    name: "wrong_mandate",
    tamper: "wrong_mandate",
    expect: "ATTESTATION_INVALID",
    attestations: [att({ signOver: { mandate: "other" } })],
  },
  {
    name: "wrong_destination",
    tamper: "wrong_destination",
    expect: "ATTESTATION_INVALID",
    attestations: [att({ signOver: { dest: "other" } })],
  },
  {
    name: "wrong_tag_other_value",
    tamper: "wrong_tag",
    expect: "ATTESTATION_INVALID",
    legs: [leg({ tag: 8n })],
    attestations: [att({ signOver: { tag: 7n } })],
  },
  {
    name: "wrong_tag_dropped",
    tamper: "wrong_tag",
    expect: "ATTESTATION_INVALID",
    legs: [leg({ tag: null })],
    attestations: [att({ signOver: { tag: 7n } })],
  },
  {
    name: "wrong_tag_zero_versus_none",
    tamper: "wrong_tag",
    expect: "ATTESTATION_INVALID",
    legs: [leg({ tag: 0n })],
    attestations: [att({ signOver: { tag: null } })],
  },
  {
    name: "wrong_policy",
    tamper: "wrong_policy",
    expect: "ATTESTATION_INVALID",
    attestations: [att({ signOver: { policy: "other" } })],
  },
  { name: "wrong_ref", tamper: "wrong_ref", expect: "ATTESTATION_INVALID", attestations: [att({ signOver: { ref: "other" } })] },
  {
    name: "wrong_valid_until",
    tamper: "wrong_valid_until",
    expect: "ATTESTATION_INVALID",
    attestations: [att({ validUntil: VU + 60n, signOver: { validUntil: VU } })],
  },
  { name: "truncated_64_bytes", tamper: "truncated", expect: "ATTESTATION_INVALID", attestations: [att({ mutate: "truncate" })] },
  { name: "extended_66_bytes", tamper: "malformed", expect: "ATTESTATION_INVALID", attestations: [att({ mutate: "extend" })] },
  { name: "empty_signature", tamper: "malformed", expect: "ATTESTATION_INVALID", attestations: [att({ mutate: "empty" })] },
  { name: "r_zero", tamper: "malformed", expect: "ATTESTATION_INVALID", attestations: [att({ mutate: "r_zero" })] },
  { name: "s_zero", tamper: "malformed", expect: "ATTESTATION_INVALID", attestations: [att({ mutate: "s_zero" })] },
  { name: "r_equals_order", tamper: "malformed", expect: "ATTESTATION_INVALID", attestations: [att({ mutate: "r_order" })] },
  {
    name: "s_half_order_is_low",
    tamper: "malformed",
    expect: "ATTESTATION_INVALID",
    attestations: [att({ mutate: "s_half_order" })],
  },
  {
    name: "s_half_order_plus_one_is_high",
    tamper: "high_s",
    expect: "ATTESTATION_INVALID",
    attestations: [att({ mutate: "s_half_order_plus_one" })],
  },
  {
    name: "r_not_on_curve",
    tamper: "malformed",
    expect: "ATTESTATION_INVALID",
    attestations: [att({ mutate: "r_not_on_curve" })],
  },
  {
    name: "recovers_to_infinity",
    tamper: "malformed",
    expect: "ATTESTATION_INVALID",
    attestations: [att({ mutate: "recovers_to_infinity" })],
  },
  { name: "expired_at_valid_until", tamper: "expired", expect: "ATTESTATION_EXPIRED", now: VU },
  { name: "expired_after_valid_until", tamper: "expired", expect: "ATTESTATION_EXPIRED", now: VU + 86400n },
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
    expect: "ATTESTATION_INVALID",
    legs: [leg({ tag: 1n, amount: 300n }), leg({ tag: 2n, amount: 200n })],
    attestations: [att({ signOver: { tag: 2n } }), att({ signOver: { tag: 1n } })],
  },
  {
    name: "second_leg_wrong_destination",
    tamper: "wrong_destination",
    expect: "ATTESTATION_INVALID",
    legs: [leg({ amount: 300n }), leg({ dest: "other", amount: 200n })],
    attestations: [att(), att({ signOver: { dest: "primary" } })],
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
    expect: "ATTESTATION_INVALID",
    now: VU,
    legs: [leg({ amount: 300n }), leg({ dest: "other", amount: 200n })],
    attestations: [att({ validUntil: VU + 60n, signer: "other" }), att()],
  },
];

function mutate(sig, spec, presentedDigest) {
  if (spec === null) return sig;
  if (Array.isArray(spec)) return MUTATIONS[spec[0]](spec[1])(sig, presentedDigest);
  return MUTATIONS[spec](sig, presentedDigest);
}

function buildCase(c) {
  const now = c.now ?? NOW;
  const ctx = { ...CTX, ...(c.ctx ?? {}) };
  const ref = c.ref ?? "primary";
  const legs = c.legs ?? [leg()];
  const atts = c.attestations ?? legs.map(() => att());
  const attesterPub = pub(KEYS.attester);

  const chains = {};
  const decisions = {};
  for (const chain of ["soroban", "celo"]) {
    const presented = chainContext(chain, ctx);
    const chainLegs = legs.map((l) => chainLeg(chain, l));
    const pairs = [];
    const outAtts = atts.map((a, i) => {
      const legSpec = legs[Math.min(i, legs.length - 1)];
      const so = a.signOver;
      const signedCtx = chainContext(chain, {
        chain: so.chain ?? ctx.chain,
        mandate: so.mandate ?? ctx.mandate,
        policy: so.policy ?? ctx.policy,
      });
      const signedLeg = chainLeg(chain, {
        ...legSpec,
        dest: so.dest ?? legSpec.dest,
        tag: "tag" in so ? so.tag : legSpec.tag,
      });
      const signedDigest = digest({
        chainTag: signedCtx.chainTag,
        recipientRef: REF[so.ref ?? ref],
        policyHash: POLICY[so.policy ?? ctx.policy],
        validUntil: so.validUntil ?? a.validUntil,
        mandateBytes: signedCtx.mandateBytes,
        destBytes: signedLeg.destBytes,
        hasTag: signedLeg.hasTag,
        tag: signedLeg.tag,
      });
      // What the contract will hash: its own context, the presented leg at the
      // same index, the presented valid_until. Null when there is no leg.
      const l = chainLegs[i];
      const presentedDigest = l
        ? digest({
            chainTag: presented.chainTag,
            recipientRef: REF[ref],
            policyHash: POLICY[ctx.policy],
            validUntil: a.validUntil,
            mandateBytes: presented.mandateBytes,
            destBytes: l.destBytes,
            hasTag: l.hasTag,
            tag: l.tag,
          })
        : null;
      const signature = mutate(sign(signedDigest, KEYS[a.signer]), a.mutate, presentedDigest ?? signedDigest);
      if (presentedDigest) pairs.push({ validUntil: a.validUntil, signature, digest: presentedDigest });
      return {
        valid_until: a.validUntil.toString(),
        signature: hex(signature),
        signed_digest: hex(signedDigest),
        digest: presentedDigest ? hex(presentedDigest) : null,
      };
    });
    decisions[chain] = referenceVerify(now, attesterPub, pairs, legs.length, atts.length);
    chains[chain] = {
      ...presented.fields,
      ...presented.extra,
      legs: chainLegs.map((l) => l.out),
      attestations: outAtts,
    };
  }

  const { soroban, celo } = decisions;
  if (soroban.code !== celo.code) throw new Error(`${c.name}: reference decision differs per chain`);
  if (soroban.code !== c.expect) throw new Error(`${c.name}: expected ${c.expect}, reference says ${soroban.code}`);
  return {
    name: c.name,
    tamper: c.tamper,
    expect: c.expect,
    expect_code: c.expect === "ok" ? 0 : ERR[c.expect],
    soroban_host_abort: soroban.hostAbort,
    calls: c.calls ?? 1,
    now: now.toString(),
    recipient_ref: hex(REF[ref]),
    country_policy_hash: hex(POLICY[ctx.policy]),
    attester_public_key: hex(attesterPub),
    attester_evm_address: evmAddress(attesterPub),
    soroban: chains.soroban,
    celo: chains.celo,
  };
}

function build() {
  const attesterPub = pub(KEYS.attester);
  const otherPub = pub(KEYS.other);
  return {
    warning:
      "TEST ONLY. Every key in this file is derived from a public label and must never hold funds, sign anything real or be configured as an attester outside tests.",
    format_version: 1,
    domain: DOMAIN,
    max_attestation_validity: MAX_ATTESTATION_VALIDITY.toString(),
    error_codes: ERR,
    keys: {
      attester: {
        derivation: 'keccak256("UJIRA/TEST-ONLY/S18/attester")',
        private_key: hex(KEYS.attester),
        public_key: hex(attesterPub),
        evm_address: evmAddress(attesterPub),
      },
      other_signer: {
        derivation: 'keccak256("UJIRA/TEST-ONLY/S18/other-signer")',
        private_key: hex(KEYS.other),
        public_key: hex(otherPub),
        evm_address: evmAddress(otherPub),
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
