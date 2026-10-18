# Spike: attestation verification parity on Soroban

Result: parity holds. The Soroban prototype computes byte-identical digests to the reference signer and makes the declared accept or reject decision on all 47 cases of the fixture, with one difference in how a rejection is reported (see "Host traps on Soroban"). This needs four small spec changes, listed at the end.

## Test-only key

Every key in `attestation-parity/fixture.json` is derived from a public label, for example `keccak256("UJIRA/TEST-ONLY/S18/attester")`, so anyone can rebuild it. These keys must never hold funds, sign anything real, or be configured as an attester outside tests. The attester's uncompressed public key is:

- `0x04678799abacf2917bc7de0b5d890d32fe8aa13ded48b835503eee57dd4ffa23a6c80aa9add479996c4fd6e3965914054d4ff152763615edd2f8c1cc697102356c`

It appears only under `spikes/`, in the fixture and in this file. The test suite reads it from the fixture.

## What was built

| Path | What it is |
| --- | --- |
| `spikes/attestation-parity/fixture.json` | Fixture: keys, digest inputs, expected digests, signatures, expected decisions |
| `spikes/attestation-parity/generator/` | Reference signer that builds the fixture (Node, `@noble/curves`, `@noble/hashes`, `@stellar/stellar-base`, all pinned). It has its own verifier and refuses to write a case whose declared outcome disagrees with it |
| `soroban/spikes/attestation-parity/` | Soroban prototype (`verify`, `digest`, plus `verify_ed25519` for comparison only) and its tests |

Run:

```sh
cd soroban && cargo test -p attestation-parity
cd spikes/attestation-parity/generator && npm ci && node generate.mjs --check ../fixture.json
```

To add a case, add it to `CASES` in `generate.mjs` and run `node generate.mjs ../fixture.json`. The suite needs no code change. This was tried with a case signed for the other network and the other mandate: the suite rejected it with code 51 and no code was touched.

## Digest

One secp256k1 key signs a raw 32 byte digest, with no message prefix and no typed struct:

```
digest = keccak256(
    "UJIRA/ATTEST/V1"             15 ASCII bytes
 || chain_tag                     32
 || recipient_ref                 32
 || country_policy_hash           32
 || u64be(valid_until)            8
 || u8(len(mandate)) || mandate
 || u8(len(dest))    || dest
 || u8(has_tag)      || u64be(tag)   has_tag is 0 or 1; tag is 0 when absent
)
signature = r[32] || s[32] || v[1], v in {27, 28}, 1 <= r < n, 1 <= s <= floor(n / 2)
```

| Field | Soroban |
| --- | --- |
| `chain_tag` | `network_id`, the SHA-256 of the network passphrase |
| `mandate` | `Address::to_xdr` of the contract: 40 bytes (`ScVal` address, contract arm) |
| `dest` | `Address::to_xdr` of `MuxedAddress::address()`: 44 bytes for a `G`, 40 for a `C` |
| `has_tag`, `tag` | from `MuxedAddress::id()`: an `M` address gives `1` and its id |

The encoding has no ambiguity: every variable field has a length prefix, and the tag has its own presence byte, so tag 0 and no tag give different digests (case `wrong_tag_zero_versus_none`). The same address with a `G` or an `M` form gives the same `dest` bytes, so only the tag tells them apart. That matches how the allowlist keys destinations.

### How the off-chain signer produces it

1. Take `recipient_ref`, `country_policy_hash` and `valid_until` from the payout.
2. `chain_tag = sha256(passphrase)`; `mandate = Address(mandate C strkey).toScVal().toXDR()`; `dest` is the same encoding of the base `G` or `C` address. If the destination is an `M` address, use its base `G` and set `has_tag = 1`, `tag = muxed id`.
3. Sign the digest as a prehashed message (`prehash: false`), with low s and RFC 6979 nonces. Output `r || s || (27 + recovery_id)`. Never output `v` as 0 or 1.

A KMS that signs raw secp256k1 digests works without any change. `generate.mjs` is the reference implementation.

### The vector format

Each case in the fixture has `name`, `tamper`, `expect` (`ok`, `ATTESTATION_INVALID` or `ATTESTATION_EXPIRED`), `expect_code`, `soroban_host_abort`, `calls`, `now`, `recipient_ref`, `country_policy_hash` and `attester_public_key`. It also has a `soroban` block with the verifier's context (network passphrase, chain tag, mandate), the legs, and per attestation `valid_until`, `signature`, `signed_digest` (what was signed) and `digest` (what the contract must compute). The `tamper` values are the names the shared vector runner uses: `wrong_signer`, `high_s`, `bad_v`, `wrong_chain`, `wrong_mandate`, `wrong_destination`, `wrong_policy`, `wrong_ref`, `wrong_tag`, `truncated`. The fixture also uses `malformed`, `expired`, `window`, `count`, `wrong_valid_until`, `wrong_amount` and `replayed`.

## Check order

1. Attestation count differs from leg count: 51.
2. For each leg, in order: `now >= valid_until`: 52; `valid_until - now > 86400`: 51; signature not 65 bytes: 51; `v` not 27 or 28: 51; `r` or `s` out of range or `s` high: 51; recovered key differs from the attester: 51.

The window check subtracts after the expiry check, so it never overflows. The contract never computes `now + 86400`.

## Results

Results come from `cargo test` and again from the compiled wasm (all 47 cases, same decisions). "cpu" is the metered Soroban instruction count of the whole `verify` call on the compiled wasm in the local test host.

| Case | Tamper | Legs/atts | Soroban | Soroban cpu |
| --- | --- | --- | --- | --- |
| `valid_untagged` | none | 1/1 | accept | 2877435 |
| `valid_tagged` | none | 1/1 | accept | 2880660 |
| `valid_tag_zero` | none | 1/1 | accept | 2880660 |
| `valid_tag_u64_max` | none | 1/1 | accept | 2882079 |
| `valid_contract_destination` | none | 1/1 | accept | 2877029 |
| `valid_two_legs_same_address_two_tags` | none | 2/2 | accept | 5282092 |
| `valid_window_upper_bound` | none | 1/1 | accept | 2877435 |
| `valid_last_second` | none | 1/1 | accept | 2877435 |
| `amount_not_bound_one` | wrong_amount | 1/1 | accept | 2877435 |
| `amount_not_bound_i128_max` | wrong_amount | 1/1 | accept | 2879272 |
| `replay_same_attestation` | replayed | 1/1 | accept | 2877435 |
| `wrong_signer` | wrong_signer | 1/1 | reject 51 | 2871395 |
| `high_s` | high_s | 1/1 | reject 51 | 494660 |
| `bad_v_0` | bad_v | 1/1 | reject 51 | 487610 |
| `bad_v_1` | bad_v | 1/1 | reject 51 | 487610 |
| `bad_v_26` | bad_v | 1/1 | reject 51 | 487610 |
| `bad_v_29` | bad_v | 1/1 | reject 51 | 487610 |
| `bad_v_30` | bad_v | 1/1 | reject 51 | 487610 |
| `bad_v_255` | bad_v | 1/1 | reject 51 | 487610 |
| `wrong_chain` | wrong_chain | 1/1 | reject 51 | 2871395 |
| `wrong_mandate` | wrong_mandate | 1/1 | reject 51 | 2871395 |
| `wrong_destination` | wrong_destination | 1/1 | reject 51 | 2871395 |
| `wrong_tag_other_value` | wrong_tag | 1/1 | reject 51 | 2874620 |
| `wrong_tag_dropped` | wrong_tag | 1/1 | reject 51 | 2871395 |
| `wrong_tag_zero_versus_none` | wrong_tag | 1/1 | reject 51 | 2874620 |
| `wrong_policy` | wrong_policy | 1/1 | reject 51 | 2871395 |
| `wrong_ref` | wrong_ref | 1/1 | reject 51 | 2871395 |
| `wrong_valid_until` | wrong_valid_until | 1/1 | reject 51 | 2871395 |
| `truncated_64_bytes` | truncated | 1/1 | reject 51 | 487195 |
| `extended_66_bytes` | malformed | 1/1 | reject 51 | 487195 |
| `empty_signature` | malformed | 1/1 | reject 51 | 487195 |
| `r_zero` | malformed | 1/1 | reject 51 | 497412 |
| `s_zero` | malformed | 1/1 | reject 51 | 497864 |
| `r_equals_order` | malformed | 1/1 | reject 51 | 497492 |
| `s_half_order_is_low` | malformed | 1/1 | reject 51 | 2874679 |
| `s_half_order_plus_one_is_high` | high_s | 1/1 | reject 51 | 497884 |
| `r_not_on_curve` | malformed | 1/1 | reject, host trap Crypto InvalidInput | 2873095 |
| `recovers_to_infinity` | malformed | 1/1 | reject, host trap Crypto InvalidInput | 2873095 |
| `expired_at_valid_until` | expired | 1/1 | reject 52 | 486796 |
| `expired_after_valid_until` | expired | 1/1 | reject 52 | 486796 |
| `window_too_long` | window | 1/1 | reject 51 | 486780 |
| `missing_attestation` | count | 1/0 | reject 51 | 443480 |
| `extra_attestation` | count | 1/2 | reject 51 | 443480 |
| `attestations_swapped_between_legs` | wrong_tag | 2/2 | reject 51 | 2874620 |
| `second_leg_wrong_destination` | wrong_destination | 2/2 | reject 51 | 5269602 |
| `first_leg_expired_wins_over_second_bad_signature` | expired | 2/2 | reject 52 | 486796 |
| `first_leg_bad_signature_wins_over_second_expired` | wrong_signer | 2/2 | reject 51 | 2871395 |

The suite also asserts, for every attestation that has a leg, that the contract's own digest equals the fixture's `digest`.

What the "accept" rows mean:

- `wrong_amount` (`amount_not_bound_*`): the amount is not in the digest, and neither is the fee. An attestation says a destination is eligible; it does not authorize a payment. Amounts are bounded by the caps and approvals.
- `replayed`: the same attestation is accepted again within its window, on purpose. Replay of a payout is stopped by the payout ID, not by the attestation. Replay to another network, mandate, destination, tag, recipient or policy is rejected (rows above).

### Mutation check

Each explicit check was removed in turn and the fixture run again:

| Removed | Effect |
| --- | --- |
| low-s rule | `high_s` traps in the host instead of returning 51 |
| strict `v` (accepting 0 and 1 as many libraries do) | `bad_v_0` accepted, while the reference verifier rejects it |

## Host traps on Soroban

`secp256k1_recover` traps instead of returning an error: `Error(Crypto, InvalidInput)`. A contract cannot catch it. The host traps on:

- `r` or `s` equal to 0, or at least `n`;
- high `s`;
- a recovery id above 3;
- an `r` that is not the x coordinate of a curve point;
- a recovered point at infinity.

The prototype checks the first three itself, so those inputs get contract error 51. The last two stay. To reject them the contract would need a curve-membership check (a modular square root) or a separate verifier contract called with `try_invoke`. The fixture covers both: `r_not_on_curve` (r = 5) and `recovers_to_infinity` (s = 1, R = e*G). After the prechecks, these two conditions are the only ways recovery can still fail. Only the two fixture cases were tested.

Neither input can come from an honest attester. Only the submitter, the agent, can build them, and the effect is that its own call fails. The decision is a reject, but it is reported as a host error rather than as error 51. Inside a batch request this aborts the whole call, not only the item.

The same was checked on testnet: a high-s signature returns `Error(Contract, #51)` and an `r` not on the curve gives `Error(Crypto, InvalidInput)` from `recover_key_ecdsa_secp256k1`.

## Cost per leg

Soroban: measured on the compiled wasm, locally and on testnet (protocol 29). Instructions are from `simulateTransaction`.

| Soroban `verify` | Testnet instructions | Local wasm cpu | Min resource fee (stroops) |
| --- | --- | --- | --- |
| call with no legs | 577,407 | | 6,473 |
| 1 leg, untagged | 3,057,593 | 2,877,435 | 7,767 |
| 1 leg, tagged | 3,061,089 | 2,880,660 | 7,777 |
| 2 legs | 5,573,675 | 5,282,092 | 9,089 |
| 3 legs | 8,086,261 | | 10,400 |
| 4 legs | 10,598,846 | | 11,711 |

Each extra leg costs 2,512,586 instructions on testnet and 2,404,657 locally, about 1,311 stroops of resource fee. The testnet transaction limit is 400,000,000 instructions, so 4 attested legs use about 2.6 percent of a transaction, and a batch of 20 four-leg items about 50 percent before any other work. The batch-size spike must use these numbers.

### ed25519 for comparison

On Soroban, `ed25519_verify` over the same 32 byte digest costs 962,819 instructions per call on testnet, against 3,057,593 for secp256k1. That is about 385,000 instructions of marginal cost per leg, against 2.5 million. The host uses strict verification, and the Soroban suite shows that each of these traps:

- a wrong message;
- a non-canonical `S` (S + L);
- the small-order identity key with R = identity and S = 0, which a lax verifier accepts for any message (the test shows that too);
- a non-canonical key encoding;
- a damaged `R`.

This spike kept secp256k1 for the attestation format. Moving to ed25519 would be cheaper per leg; that is a spec change and is not made here.

## Spec changes this spike needs

1. Soroban `Attestation.signature` should be `Bytes`, not `BytesN<65>`, with an explicit length check. With `BytesN<65>`, a 64 or 66 byte signature fails argument decoding before the contract runs, so the contract could never return 51 for `truncated`.
2. The rule for malformed signatures should name the explicit prechecks the contract runs before recovery: length 65, `v` in {27, 28}, `1 <= r < n`, `1 <= s <= floor(n / 2)`. These are required: without them, high s traps, and accepting `v` of 0 or 1 accepts signatures the reference verifier rejects.
3. Signatures that pass the prechecks but cannot be recovered (`r` not on the curve, result at infinity) abort the call with a host error. The spec should say so, and the vector runner should expect a host abort for these two tampers (`soroban_host_abort` in the fixture). For batch requests, such an item aborts the whole batch.
4. Add the window check form `valid_until - now > MAX_ATTESTATION_VALIDITY`, evaluated after the expiry check. It is the same rule as `valid_until <= now + MAX` but it cannot overflow.

No byte layout change is needed.
