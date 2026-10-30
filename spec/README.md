# spec

The mandate rules, written once and independent of any chain or language, plus the JSON test vectors every implementation must pass.

A mandate is the set of limits a principal puts on an agent key: total budget, per-payment cap, per-recipient daily cap, allowed recipients or countries, approval threshold and expiry, along with the principal's ability to pause or revoke it at any time.

What belongs here:

- The written specification of each rule and how the rules combine.
- Test vectors in JSON: a starting mandate state, a sequence of actions and the expected outcome of each one.

The contract in `soroban/` implements this spec. When the spec and a contract disagree, the contract is wrong.

Scope: spec version 1 is in force on Stellar only. Rules and text marked Celo or EVM are kept for reference and are not in force. An EVM chain added later needs a spec revision that defines its attestation scheme (for example secp256k1 with `ecrecover` over the same SHA-256 digest) and its bad-signature behaviour, and must not run `ATTESTED` or `BOTH` mandates until then.

## Files

- `MANDATE.md`: the rules, each with a stable ID (`MANDATE-*` for the contracts, `CHAINOPS-*` for the off-chain services), the check order of every operation, the invariants and the rule index.
- `errors.json`: the error table as `{ name, code, summary }`, append only; codes are never reused.
- `constants.json`: the constants, the role codes and the recovery action kinds.
- `EVENTS.md`: the canonical events with their fields and their encoding on Stellar.
- `ENCODING.md`: the byte layouts of the IDs and of the version 2 attestation message, with a worked example that `scripts/attestation-message.py --example` recomputes.
- `retired-ids.txt`: the rule IDs that are retired. A retired rule stays in `MANDATE.md` with a title starting `Retired:`, and its ID is never deleted or reused.
- `vectors/`: the shared test vectors, their JSON Schema (`vectors/schema.json`) and a complete example. `vectors/README.md` explains the format, and `scripts/check-vectors.sh` checks every vector file against the schema and this spec.

`scripts/check-spec-ids.sh --check-order` checks that every rule ID is unique, defined and indexed, that every rule area is covered, that every error a rule cites is in the error table, and that every numbered check cites exactly one rule. `scripts/check-spec-ids.test.sh` is its self test. CI runs both.
