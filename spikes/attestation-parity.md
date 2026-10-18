# Spike: attestation verification parity on Soroban

Result: parity holds. The Soroban prototype builds byte-identical attested fields and messages to the reference signer and makes the declared decision on all 48 cases of the fixture: 13 accepted, 15 refused with a contract error, and 20 refused by a host trap in ed25519 verification. A verified leg costs 496,907 instructions on top of the call.

Run: 2026-10-18, on development at `28922b0` plus this change. The measured contract is `soroban/spikes/attestation-parity/fixtures/attestation_parity.wasm`, sha256 `3286a4ae6608f88c1604448eb6233de9c23c7fb462a74968a967aa88a8e27666`, built with Rust 1.98.1, soroban-sdk 29.0.1 and stellar-cli 28.1.0. Figures are from the local test host (soroban-env-host 29.0.1), not from testnet.

History: until 2026-10-18 this spike used version 1 (secp256k1 over a keccak digest with prefix `UJIRA/ATTEST/V1`), at 2,877,435 instructions for one leg; the spec moved attestations to ed25519 and version 1 was never deployed.

## Test-only keys

Every key in `attestation-parity/fixture.json` is an ed25519 key whose 32 byte seed is the SHA-256 of a public label, written next to the key in the fixture, so anyone can rebuild it. These keys must never hold funds, sign anything real, or be configured as an attester outside tests. The fixture holds the labels and public keys only; a consumer that needs to sign derives the seed itself.

| Key | Seed | Public key |
| --- | --- | --- |
| attester | `sha256("UJIRA/TEST-ONLY/S18/attester-ed25519")` | `0x421adb50c10fe73fb1fc0d4211a16911a188bb001eaf0391b479316026141d48` |
| other signer | `sha256("UJIRA/TEST-ONLY/S18/other-signer-ed25519")` | `0xa51a56cfabe1fbb71d7172c52fd2bb9d715f75fb50e65b060961fb1690a59cab` |

## What was built

| Path | What it is |
| --- | --- |
| `spikes/attestation-parity/fixture.json` | Fixture: keys, case inputs, the signed and the checked fields, their SHA-256 and message, signatures, expected decisions |
| `spikes/attestation-parity/generator/` | Reference signer that builds the fixture (Node, `@noble/curves`, `@noble/hashes`, `@stellar/stellar-base`, all pinned). It has its own strict verifier and refuses to write a case whose declared outcome disagrees with it |
| `soroban/spikes/attestation-parity/` | Soroban prototype (`verify`, plus `fields` and `message` so the encoding can be compared) and its tests |
| `soroban/spikes/attestation-parity/fixtures/attestation_parity.wasm` | The prototype compiled to wasm, so costs include the VM. CI rebuilds it and fails if it differs |

Run:

```sh
cd soroban && cargo test -p attestation-parity -- --nocapture
cd spikes/attestation-parity/generator && npm ci && node generate.mjs --check ../fixture.json
```

To add a case, add it to `CASES` in `generate.mjs` and run `node generate.mjs ../fixture.json`. The suite needs no code change.

## Message

One ed25519 key signs a 47 byte message:

```
fields = network_id                          32   SHA-256 of the network passphrase
      || u8(len(mandate))  || mandate             Address::to_xdr of the mandate, 40 bytes
      || recipient_ref                       32
      || country_policy_hash                 32
      || u64be(policy_epoch)                 8
      || u64be(valid_until)                  8
      || u8(len(dest))     || dest                Address::to_xdr of the base G (44) or C (40)
      || u8(has_tag)       || u64be(tag)          has_tag is 0 or 1; tag is 0 when absent
message   = "UJIRA/ATTEST/V2" || SHA-256(fields)  15 + 32 = 47 bytes
signature = ed25519(attester, message)            64 bytes, R || S
```

With an account destination the fields are 207 bytes, with a contract destination 203. The message can never equal a 32 byte transaction hash, so an attester signature can never be replayed as a transaction signature or the other way round.

The encoding has no ambiguity: every variable field has a length prefix, and the tag has its own presence byte, so tag 0 and no tag give different messages (case `wrong_tag_zero_versus_none`). The same address in `G` or `M` form gives the same `dest` bytes, so only the tag tells them apart. That matches how the allowlist keys destinations. `policy_epoch` is the epoch the attestation names, and the contract only reaches the signature once that equals its current epoch.

Hand check: the fields, SHA-256 and message of `valid_tagged` were rebuilt in plain Python from this layout, with no code shared with the generator or the contract, and matched the fixture byte for byte.

### How the off-chain signer produces it

1. Take `recipient_ref`, `country_policy_hash`, `policy_epoch` and `valid_until` from the payout and the mandate's current policy.
2. `network_id = sha256(passphrase)`; `mandate = Address(mandate C strkey).toScVal().toXDR()`; `dest` is the same encoding of the base `G` or `C` address. If the destination is an `M` address, use its base `G` and set `has_tag = 1`, `tag = muxed id`.
3. Hash the fields with SHA-256, prepend `UJIRA/ATTEST/V2` and sign the 47 bytes with ed25519 as a plain message (no prehash). Output the 64 byte signature.

`generate.mjs` is the reference implementation.

### The vector format

Each case in the fixture has `name`, `tamper`, `expect` (`ok`, `ATTESTATION_INVALID`, `ATTESTATION_EXPIRED`, `POLICY_CHANGED` or `HOST_TRAP`), `expect_code` (null for a trap), `calls`, `now`, the network passphrase and id, the mandate and its XDR, `attester_public_key`, `recipient_ref`, `country_policy_hash`, the mandate's `policy_epoch`, and the legs. Each attestation has `valid_until`, `policy_epoch`, `signature`, `signed` (what the signer encoded) and `checked` (what the contract must build, null when there is no leg at that index), each with `fields`, `fields_sha256` and `message`.

The `tamper` values: `wrong_signer`, `wrong_chain`, `wrong_mandate`, `wrong_destination`, `wrong_policy`, `wrong_ref`, `wrong_tag`, `wrong_valid_until`, `wrong_epoch` (signed under another epoch than the one it names), `stale_epoch` (names another epoch than the mandate's), `truncated` and `malformed` (wrong length), `bad_signature` (damaged or non-canonical bytes), `expired`, `window`, `count`, `wrong_amount` and `replayed`.

## Check order

1. Attestation count differs from leg count: 51.
2. For each leg, in order: `now >= valid_until`: 52; `valid_until - now > 86400`: 51; signature not 64 bytes: 51; `policy_epoch` differs from the mandate's: 53; signature not the attester's over the message: host trap.

The window check subtracts after the expiry check, so it never overflows. The epoch check comes before the signature, so an attestation from before a policy change gets 53 for that item instead of a trap (cases `stale_policy_epoch_wins_over_bad_signature` and `second_leg_stale_policy_epoch`).

## Results

Every case runs twice, on the native contract and on the compiled wasm, with the same decision. "cpu" and "mem" are the metered instructions and memory bytes of the whole `verify` call on the wasm, including the call that traps.

| Case | Tamper | Expect | cpu | mem |
| --- | --- | --- | --- | --- |
| `valid_untagged` | none | ok | 964120 | 1264708 |
| `valid_tagged` | none | ok | 967345 | 1264812 |
| `valid_tag_zero` | none | ok | 967345 | 1264812 |
| `valid_tag_u64_max` | none | ok | 968764 | 1264916 |
| `valid_contract_destination` | none | ok | 963680 | 1264442 |
| `valid_two_legs_same_address_two_tags` | none | ok | 1461027 | 1270600 |
| `valid_window_upper_bound` | none | ok | 964120 | 1264708 |
| `valid_last_second` | none | ok | 964120 | 1264708 |
| `valid_later_policy_epoch` | none | ok | 964120 | 1264708 |
| `valid_policy_epoch_u64_max` | none | ok | 966379 | 1264836 |
| `amount_not_bound_one` | wrong_amount | ok | 964120 | 1264708 |
| `amount_not_bound_i128_max` | wrong_amount | ok | 965957 | 1264732 |
| `replay_same_attestation` | replayed | ok | 964120 | 1264708 |
| `wrong_signer` | wrong_signer | HOST_TRAP | 962428 | 1264708 |
| `wrong_chain` | wrong_chain | HOST_TRAP | 962428 | 1264708 |
| `wrong_mandate` | wrong_mandate | HOST_TRAP | 962428 | 1264708 |
| `wrong_destination` | wrong_destination | HOST_TRAP | 962428 | 1264708 |
| `wrong_tag_other_value` | wrong_tag | HOST_TRAP | 965653 | 1264812 |
| `wrong_tag_dropped` | wrong_tag | HOST_TRAP | 962428 | 1264708 |
| `wrong_tag_zero_versus_none` | wrong_tag | HOST_TRAP | 965653 | 1264812 |
| `wrong_policy` | wrong_policy | HOST_TRAP | 962428 | 1264708 |
| `wrong_ref` | wrong_ref | HOST_TRAP | 962428 | 1264708 |
| `wrong_valid_until` | wrong_valid_until | HOST_TRAP | 962428 | 1264708 |
| `wrong_policy_epoch_signed` | wrong_epoch | HOST_TRAP | 962428 | 1264708 |
| `stale_policy_epoch` | stale_epoch | POLICY_CHANGED | 482348 | 1259192 |
| `future_policy_epoch` | stale_epoch | POLICY_CHANGED | 482348 | 1259192 |
| `stale_policy_epoch_wins_over_bad_signature` | stale_epoch | POLICY_CHANGED | 482348 | 1259192 |
| `signature_63_bytes` | truncated | ATTESTATION_INVALID | 482332 | 1259192 |
| `signature_65_bytes` | malformed | ATTESTATION_INVALID | 482332 | 1259192 |
| `empty_signature` | malformed | ATTESTATION_INVALID | 482332 | 1259192 |
| `short_signature_wins_over_stale_epoch` | truncated | ATTESTATION_INVALID | 482332 | 1259192 |
| `flipped_r_bit` | bad_signature | HOST_TRAP | 962428 | 1264708 |
| `flipped_s_bit` | bad_signature | HOST_TRAP | 962428 | 1264708 |
| `non_canonical_s` | bad_signature | HOST_TRAP | 962428 | 1264708 |
| `zero_signature` | bad_signature | HOST_TRAP | 962428 | 1264708 |
| `identity_r_zero_s` | bad_signature | HOST_TRAP | 962428 | 1264708 |
| `small_order_attester_key` | bad_signature | HOST_TRAP | 962428 | 1264708 |
| `expired_at_valid_until` | expired | ATTESTATION_EXPIRED | 481933 | 1259192 |
| `expired_after_valid_until` | expired | ATTESTATION_EXPIRED | 481933 | 1259192 |
| `expired_wins_over_stale_epoch` | expired | ATTESTATION_EXPIRED | 481933 | 1259192 |
| `window_too_long` | window | ATTESTATION_INVALID | 481917 | 1259192 |
| `missing_attestation` | count | ATTESTATION_INVALID | 426513 | 1257414 |
| `extra_attestation` | count | ATTESTATION_INVALID | 426513 | 1257414 |
| `attestations_swapped_between_legs` | wrong_tag | HOST_TRAP | 965653 | 1264812 |
| `second_leg_wrong_destination` | wrong_destination | HOST_TRAP | 1452885 | 1270392 |
| `second_leg_stale_policy_epoch` | stale_epoch | POLICY_CHANGED | 972805 | 1264876 |
| `first_leg_expired_wins_over_second_bad_signature` | expired | ATTESTATION_EXPIRED | 481933 | 1259192 |
| `first_leg_bad_signature_wins_over_second_expired` | wrong_signer | HOST_TRAP | 962428 | 1264708 |

The suite also asserts, for every attestation with a leg, that the contract's own `fields` and `message` equal the fixture's `checked` block, that each `fields_sha256` and `message` follows from its `fields`, and that every trap is `Error(Crypto, InvalidInput)` with "failed ED25519 verification" in the diagnostics.

What the "accept" rows mean:

- `wrong_amount` (`amount_not_bound_*`): the amount is not in the message, and neither is the fee. An attestation says a destination is eligible; it does not authorize a payment. Amounts are bounded by the caps and approvals.
- `replayed`: the same attestation is accepted again within its window, on purpose. Replay of a payout is stopped by the payout ID, not by the attestation. Replay to another network, mandate, destination, tag, recipient, policy or epoch is refused (rows above).

### Mutation check

Each change below was made to the contract in turn and the fixture run again:

| Change | Effect |
| --- | --- |
| tag presence byte dropped from the fields | `valid_untagged` fails on its fields at the first case |
| epoch checked after the signature instead of before | `stale_policy_epoch_wins_over_bad_signature` traps instead of returning 53 |

## Host traps

`ed25519_verify` traps on any failure, `Error(Crypto, InvalidInput)`, and a contract cannot catch it. The host verifies strictly, and the suite shows each of these traps:

- a signature by another key, or over other fields;
- a flipped bit in R or in S;
- a non-canonical S (S + L), which would make signatures malleable if accepted;
- an all-zero signature, and R = identity with S = 0;
- the small-order identity key with R = identity and S = 0, which a lax, cofactorless check accepts for any message (the suite shows that too);
- a non-canonical key encoding (y = p).

The contract only checks that the attester is 32 bytes and not all zero, so a small-order or non-canonical key is accepted at construction and every attestation against it traps. Refusing such keys is the operator's job before they are configured.

Only the submitter, the agent, can present a bad signature, and the effect is that its own call fails. Inside a batch request this aborts the whole call, not only the item.

## Cost per leg

| `verify` on the wasm | cpu | mem |
| --- | --- | --- |
| refused at the count check (no verification) | 426,513 | 1,257,414 |
| 1 leg, untagged | 964,120 | 1,264,708 |
| 1 leg, tagged | 967,345 | 1,264,812 |
| 2 legs | 1,461,027 | 1,270,600 |
| 1 leg, trap in verification | 962,428 | 1,264,708 |

A verified leg costs 496,907 instructions on top of the call, which covers building the fields, their SHA-256 and the ed25519 verification. The split between those was not measured. A trap costs the same as a verified leg, because it happens in the same verification. Refusals before verification (expiry, window, length, epoch) cost about 482,000 for the call.
