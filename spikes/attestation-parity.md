# Spike: attestation verification parity on Soroban and Celo

Result: parity holds. Both chains compute byte-identical digests from the shared fixture and make the same accept or reject decision on all 47 cases, with one difference in how a rejection is reported (see "Host traps on Soroban"). This needs four small spec changes, listed at the end.

## Test-only key

Every key in `attestation-parity/fixture.json` is derived from a public label, for example `keccak256("UJIRA/TEST-ONLY/S18/attester")`, so anyone can rebuild it. These keys must never hold funds, sign anything real, or be configured as an attester outside tests. The attester's public values are:

- uncompressed key `0x04678799abacf2917bc7de0b5d890d32fe8aa13ded48b835503eee57dd4ffa23a6c80aa9add479996c4fd6e3965914054d4ff152763615edd2f8c1cc697102356c`
- EVM address `0x0eb91e2fdbcee9a4599b08b0362721cb7b9dd896`

They appear only under `spikes/`, in the fixture and in this file. The test suites read them from the fixture.

## What was built

| Path | What it is |
| --- | --- |
| `spikes/attestation-parity/fixture.json` | Shared fixture: keys, digest inputs per chain, expected digests, signatures, expected decisions |
| `spikes/attestation-parity/generator/` | Reference signer that builds the fixture (Node, `@noble/curves`, `@noble/hashes`, `@stellar/stellar-base`, all pinned). It has its own verifier and refuses to write a case whose declared outcome disagrees with it |
| `soroban/spikes/attestation-parity/` | Soroban prototype (`verify`, `digest`, plus `verify_ed25519` for comparison only) and its tests |
| `celo/test/spikes/AttestationParityVerifier.sol` | Celo prototype, kept under `test/` so it can never ship |
| `celo/test/spikes/AttestationParity.t.sol` | Fixture suite, fuzz tests and a forgery invariant |

Run:

```sh
cd soroban && cargo test -p attestation-parity
cd celo && forge test --match-contract AttestationParity
cd spikes/attestation-parity/generator && npm ci && node generate.mjs --check ../fixture.json
```

To add a case, add it to `CASES` in `generate.mjs` and run `node generate.mjs ../fixture.json`. Neither suite needs a code change. This was tried with a case signed for the other chain and the other mandate: both suites rejected it with code 51 and no code was touched.

## Digest

One secp256k1 key signs a raw 32 byte digest, with no EIP-191 prefix and no EIP-712 struct:

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

| Field | Soroban | Celo |
| --- | --- | --- |
| `chain_tag` | `network_id`, the SHA-256 of the network passphrase | `u256be(block.chainid)` |
| `mandate` | `Address::to_xdr` of the contract: 40 bytes (`ScVal` address, contract arm) | 20 raw bytes of `address(this)` |
| `dest` | `Address::to_xdr` of `MuxedAddress::address()`: 44 bytes for a `G`, 40 for a `C` | 20 raw bytes |
| `has_tag`, `tag` | from `MuxedAddress::id()`: an `M` address gives `1` and its id | `Leg.hasTag`, `Leg.tag` |

The encoding has no ambiguity: every variable field has a length prefix, and the tag has its own presence byte, so tag 0 and no tag give different digests (case `wrong_tag_zero_versus_none`). The same address with a `G` or an `M` form gives the same `dest` bytes, so only the tag tells them apart. That matches how the allowlist keys destinations.

### How the off-chain signer produces both

The scheme is the same on both chains: one key, one curve, the same hash and the same signature bytes. Only the inputs change per chain, so a payout that may land on either chain needs two attestations, one per digest.

1. Take `recipient_ref`, `country_policy_hash` and `valid_until` from the payout. They are identical for both chains.
2. Soroban: `chain_tag = sha256(passphrase)`; `mandate = Address(mandate C strkey).toScVal().toXDR()`; `dest` is the same encoding of the base `G` or `C` address. If the destination is an `M` address, use its base `G` and set `has_tag = 1`, `tag = muxed id`.
3. Celo: `chain_tag = u256be(chain_id)`; `mandate` and `dest` are the 20 address bytes; `has_tag` and `tag` are as stored on the leg.
4. Sign the digest as a prehashed message (`prehash: false`, or `--no-hash` in `cast wallet sign`), with low s and RFC 6979 nonces. Output `r || s || (27 + recovery_id)`. Never output `v` as 0 or 1.

A KMS that signs raw secp256k1 digests works without any change. `generate.mjs` is the reference implementation.

### The shared vector format

Each case in the fixture has `name`, `tamper`, `expect` (`ok`, `ATTESTATION_INVALID` or `ATTESTATION_EXPIRED`), `expect_code`, `soroban_host_abort`, `calls`, `now`, `recipient_ref`, `country_policy_hash`, `attester_public_key`, `attester_evm_address`. It also has one block per chain with the verifier's context (chain tag, mandate), the legs as the chain types them, and per attestation `valid_until`, `signature`, `signed_digest` (what was signed) and `digest` (what the contract must compute). The `tamper` values are the names the shared vector runner uses: `wrong_signer`, `high_s`, `bad_v`, `wrong_chain`, `wrong_mandate`, `wrong_destination`, `wrong_policy`, `wrong_ref`, `wrong_tag`, `truncated`. The fixture also uses `malformed`, `expired`, `window`, `count`, `wrong_valid_until`, `wrong_amount` and `replayed`.

## Check order (both chains)

1. Attestation count differs from leg count: 51.
2. For each leg, in order: `now >= valid_until`: 52; `valid_until - now > 86400`: 51; signature not 65 bytes: 51; `v` not 27 or 28: 51; `r` or `s` out of range or `s` high: 51; recovered key differs from the attester: 51.

The window check subtracts after the expiry check, so it never overflows on either chain. Neither chain computes `now + 86400`.

## Results

Soroban results come from `cargo test` and again from the compiled wasm (all 47 cases, same decisions). Celo results come from `forge test`. "cpu" is the metered Soroban instruction count of the whole `verify` call on the compiled wasm in the local test host.

| Case | Tamper | Legs/atts | Soroban | Celo | Soroban cpu |
| --- | --- | --- | --- | --- | --- |
| `valid_untagged` | none | 1/1 | accept | accept | 2877435 |
| `valid_tagged` | none | 1/1 | accept | accept | 2880660 |
| `valid_tag_zero` | none | 1/1 | accept | accept | 2880660 |
| `valid_tag_u64_max` | none | 1/1 | accept | accept | 2882079 |
| `valid_contract_destination` | none | 1/1 | accept | accept | 2877029 |
| `valid_two_legs_same_address_two_tags` | none | 2/2 | accept | accept | 5282092 |
| `valid_window_upper_bound` | none | 1/1 | accept | accept | 2877435 |
| `valid_last_second` | none | 1/1 | accept | accept | 2877435 |
| `amount_not_bound_one` | wrong_amount | 1/1 | accept | accept | 2877435 |
| `amount_not_bound_i128_max` | wrong_amount | 1/1 | accept | accept | 2879272 |
| `replay_same_attestation` | replayed | 1/1 | accept | accept | 2877435 |
| `wrong_signer` | wrong_signer | 1/1 | reject 51 | reject 51 | 2871395 |
| `high_s` | high_s | 1/1 | reject 51 | reject 51 | 494660 |
| `bad_v_0` | bad_v | 1/1 | reject 51 | reject 51 | 487610 |
| `bad_v_1` | bad_v | 1/1 | reject 51 | reject 51 | 487610 |
| `bad_v_26` | bad_v | 1/1 | reject 51 | reject 51 | 487610 |
| `bad_v_29` | bad_v | 1/1 | reject 51 | reject 51 | 487610 |
| `bad_v_30` | bad_v | 1/1 | reject 51 | reject 51 | 487610 |
| `bad_v_255` | bad_v | 1/1 | reject 51 | reject 51 | 487610 |
| `wrong_chain` | wrong_chain | 1/1 | reject 51 | reject 51 | 2871395 |
| `wrong_mandate` | wrong_mandate | 1/1 | reject 51 | reject 51 | 2871395 |
| `wrong_destination` | wrong_destination | 1/1 | reject 51 | reject 51 | 2871395 |
| `wrong_tag_other_value` | wrong_tag | 1/1 | reject 51 | reject 51 | 2874620 |
| `wrong_tag_dropped` | wrong_tag | 1/1 | reject 51 | reject 51 | 2871395 |
| `wrong_tag_zero_versus_none` | wrong_tag | 1/1 | reject 51 | reject 51 | 2874620 |
| `wrong_policy` | wrong_policy | 1/1 | reject 51 | reject 51 | 2871395 |
| `wrong_ref` | wrong_ref | 1/1 | reject 51 | reject 51 | 2871395 |
| `wrong_valid_until` | wrong_valid_until | 1/1 | reject 51 | reject 51 | 2871395 |
| `truncated_64_bytes` | truncated | 1/1 | reject 51 | reject 51 | 487195 |
| `extended_66_bytes` | malformed | 1/1 | reject 51 | reject 51 | 487195 |
| `empty_signature` | malformed | 1/1 | reject 51 | reject 51 | 487195 |
| `r_zero` | malformed | 1/1 | reject 51 | reject 51 | 497412 |
| `s_zero` | malformed | 1/1 | reject 51 | reject 51 | 497864 |
| `r_equals_order` | malformed | 1/1 | reject 51 | reject 51 | 497492 |
| `s_half_order_is_low` | malformed | 1/1 | reject 51 | reject 51 | 2874679 |
| `s_half_order_plus_one_is_high` | high_s | 1/1 | reject 51 | reject 51 | 497884 |
| `r_not_on_curve` | malformed | 1/1 | reject, host trap Crypto InvalidInput | reject 51 | 2873095 |
| `recovers_to_infinity` | malformed | 1/1 | reject, host trap Crypto InvalidInput | reject 51 | 2873095 |
| `expired_at_valid_until` | expired | 1/1 | reject 52 | reject 52 | 486796 |
| `expired_after_valid_until` | expired | 1/1 | reject 52 | reject 52 | 486796 |
| `window_too_long` | window | 1/1 | reject 51 | reject 51 | 486780 |
| `missing_attestation` | count | 1/0 | reject 51 | reject 51 | 443480 |
| `extra_attestation` | count | 1/2 | reject 51 | reject 51 | 443480 |
| `attestations_swapped_between_legs` | wrong_tag | 2/2 | reject 51 | reject 51 | 2874620 |
| `second_leg_wrong_destination` | wrong_destination | 2/2 | reject 51 | reject 51 | 5269602 |
| `first_leg_expired_wins_over_second_bad_signature` | expired | 2/2 | reject 52 | reject 52 | 486796 |
| `first_leg_bad_signature_wins_over_second_expired` | wrong_signer | 2/2 | reject 51 | reject 51 | 2871395 |

Both suites also assert, for every attestation that has a leg, that the contract's own digest equals the fixture's `digest`. The Celo and Soroban digests of `valid_tagged` were also checked by hand with `cast keccak` over the concatenated bytes.

What the "accept" rows mean:

- `wrong_amount` (`amount_not_bound_*`): the amount is not in the digest, and neither is the fee. An attestation says a destination is eligible; it does not authorize a payment. Amounts are bounded by the caps and approvals.
- `replayed`: the same attestation is accepted again within its window, on purpose. Replay of a payout is stopped by the payout ID, not by the attestation. Replay to another chain, mandate, destination, tag, recipient or policy is rejected (rows above).

### Fuzz and invariant (Celo)

- Any honest signature, for random refs, destinations, tags and windows, is accepted (1,000 runs).
- The high-s twin of any honest signature recovers to the same address through `ecrecover` and is still rejected (1,000 runs).
- Changing any bound field (ref, destination, tag value, tag presence, `valid_until`) rejects (1,000 runs).
- Arbitrary 65 bytes and any other length reject (1,000 runs each).
- Expiry and the window follow the boundaries exactly for any `valid_until` (1,000 runs).
- Invariant: a handler sends 16,384 forgeries (other key, the real key over another digest, high-s twin, random bytes, `v + 2`) and none is accepted.

### Mutation check

Each explicit check was removed in turn and the shared fixture run again:

| Removed | Chain | Effect |
| --- | --- | --- |
| low-s rule | Celo | `high_s` accepted (the precompile accepts high s) |
| low-s rule | Soroban | `high_s` traps in the host instead of returning 51 |
| strict `v` (accepting 0 and 1 as many libraries do) | Soroban | `bad_v_0` accepted, while Celo rejects it |
| `v` check | Celo | no change, `ecrecover` returns zero for other `v` (kept as a guard) |
| tag presence byte | Celo | digest mismatch on every tagged case |
| count check | Celo | out-of-bounds panic instead of 51 |

## Host traps on Soroban

`secp256k1_recover` traps instead of returning an error: `Error(Crypto, InvalidInput)`. A contract cannot catch it. The host traps on:

- `r` or `s` equal to 0, or at least `n`;
- high `s`;
- a recovery id above 3;
- an `r` that is not the x coordinate of a curve point;
- a recovered point at infinity.

The prototype checks the first three itself, so those inputs get contract error 51, the same as on Celo. The last two stay. To reject them the contract would need a curve-membership check (a modular square root) or a separate verifier contract called with `try_invoke`. The fixture covers both: `r_not_on_curve` (r = 5) and `recovers_to_infinity` (s = 1, R = e*G). On Celo, `ecrecover` returns zero for both fixture cases. After the prechecks, these two conditions are the only ways recovery can still fail on either chain, so we expect the inputs that trap on Soroban to be exactly the ones that return zero on Celo. Only the two fixture cases were tested.

Neither input can come from an honest attester. Only the submitter, the agent, can build them, and the effect is that its own call fails. The decision is the same on both chains (reject), but the report differs: a host error on Soroban, `MandateError(51)` on Celo. Inside a batch request this would abort the whole call on Soroban, while Celo would only reject the item.

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

Celo: Celo Sepolia `eth_estimateGas` against the prototype code placed at the fixture's mandate address with a state override, with the attester and policy in cold storage, as in a real request. Also measured locally with `forge test`.

| Celo `verify` | Celo Sepolia, whole tx | Local, execution only (warm) | Calldata bytes |
| --- | --- | --- | --- |
| 1 leg | 36,395 | 8,598 | 516 |
| 2 legs | 45,585 | 15,769 | 868 |
| 3 legs | 55,195 | | 1,220 |
| 4 legs | 64,426 | | 1,572 |

Each extra leg costs about 9,200 to 9,600 gas on chain, including 352 calldata bytes, and 7,171 gas of execution. A tampered input on Celo Sepolia reverts with `MandateError(51)`.

### ed25519 for comparison

On Soroban, `ed25519_verify` over the same 32 byte digest costs 962,819 instructions per call on testnet, against 3,057,593 for secp256k1. That is about 385,000 instructions of marginal cost per leg, against 2.5 million. The host uses strict verification, and the Soroban suite shows that each of these traps:

- a wrong message;
- a non-canonical `S` (S + L);
- the small-order identity key with R = identity and S = 0, which a lax verifier accepts for any message (the test shows that too);
- a non-canonical key encoding;
- a damaged `R`.

ed25519 is still not recommended. Celo has no ed25519 precompile (assumed, not probed), so it would mean a second scheme, a second signer key, a second set of vectors, and a Solidity ed25519 verifier on Celo. A secp256k1 attestation is within budget on both chains.

## Spec changes this spike needs

1. Soroban `Attestation.signature` should be `Bytes`, not `BytesN<65>`, with an explicit length check. With `BytesN<65>`, a 64 or 66 byte signature fails argument decoding before the contract runs, so Soroban could never return 51 for `truncated`, while Celo does.
2. The rule for malformed signatures should name the explicit prechecks both chains run before recovery: length 65, `v` in {27, 28}, `1 <= r < n`, `1 <= s <= floor(n / 2)`. On Soroban these are required: without them, high s traps, and accepting `v` of 0 or 1 makes Soroban accept what Celo rejects.
3. Signatures that pass the prechecks but cannot be recovered (`r` not on the curve, result at infinity) abort the call on Soroban with a host error. The spec should say so. The shared runner should expect `ATTESTATION_INVALID` on Celo and a host abort on Soroban for these two tampers (`soroban_host_abort` in the fixture). For batch requests, either accept that such an item aborts the whole Soroban batch, or require Celo to revert the whole batch too, so the two chains leave the same state.
4. Add the window check form `valid_until - now > MAX_ATTESTATION_VALIDITY`, evaluated after the expiry check. It is the same rule as `valid_until <= now + MAX` but it cannot overflow on either chain.

No byte layout change is needed.
