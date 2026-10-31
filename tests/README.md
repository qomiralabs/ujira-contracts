# tests

Behavior tests that run the vectors from `spec/` against the Soroban contract.

A vector that fails is a failing build. Tests that only concern one contract belong next to that contract, in `soroban/`.

## Running the vectors

```sh
scripts/soroban-build.sh                     # builds the factory and the mandate wasm
tests/run --chain stellar                    # every vector in spec/vectors
tests/run --chain stellar --files spec/vectors/example.json --trace-out t.json
tests/run --chain stellar --only-calls create,version,config,state,payout,daily_used,is_allowed
tests/run --chain stellar --areas PAUSE,SWEEP
tests/run --chain stellar --list             # what a selection would run, without running it
tests/compare t.json                         # a trace against its vectors
tests/compare a.json b.json                  # two traces, step by step
```

`tests/run` loads every `*.json` in `spec/vectors` except `schema.json`, so a new vector file runs without any change here. For each vector that applies to the chain it builds a fresh Soroban test environment, runs the setup and every step, records a trace, and compares the trace with the vector. It prints one line per failure, naming the vector, the step and the first differing field, and a summary:

```text
FAIL vector reference_wrong_version, step 2 (version): return: expected 1, got 2
238 selected: 0 passed, 0 failed, 228 not runnable, 10 skipped for stellar
```

Selection:

- `--files a.json,b.json` runs only those files. Paths are relative to where `tests/run` is run from.
- `--areas A,B` runs only vectors that cite a rule in one of those areas.
- `--only-calls f,g` runs only vectors whose every contract function is listed. The functions the setup calls on the vector's behalf count: `create` for a mandate in the setup and `allow_recipients` for an allowlist. Step kinds such as `transfer` may be listed and never exclude a vector. An unknown name is refused.

Every filter given must match.

Exit codes: 0 every selected vector that applies to the chain ran and passed; 1 a vector failed; 2 bad usage or unreadable input; 3 nothing failed but some vectors could not run because the contract wasm is not built. A vector that cannot run is never counted as a pass.

## The Soroban driver

The driver is the `vector-runner` crate in `soroban/vector-runner`. It registers the compiled factory and mandate wasm, never a native test contract, because the host reports some errors differently through a native contract than a network does. By default it reads `soroban/target/wasm32v1-none/release/ujira_factory.wasm` and `ujira_mandate.wasm`, as `scripts/soroban-build.sh` leaves them; `--factory-wasm` and `--mandate-wasm` point it elsewhere.

CI creates mandates through the production factory in `soroban/contracts/factory` and runs every vector:

```sh
tests/run --chain stellar
```

The test-only reference factory in `soroban/vector-runner/contracts/reference-factory` stays for the runner's self tests, which prove the runner without any production contract.

For each vector it:

- sets the ledger time to `setup.now`, the network to the testnet passphrase, and testnet's entry TTLs; every later time step also moves the ledger sequence on by one ledger per 5 seconds, so records archive as they would on a network;
- creates each actor once: an account whose ed25519 key has the seed `SHA-256("ujira-test-actor:" || name)`, or for an actor listed as a contract, a contract at that seed that accepts any signature;
- issues `asset` as a Stellar Asset Contract whose issuer can freeze and claw back, and `other_token` as a second Stellar Asset Contract, or as the test token in `soroban/vector-runner/contracts/test-token` for the `moves_settlement` and `returns_value` kinds; every account starts with an authorized trustline for both;
- registers the factory with the mandate's wasm hash, the supported asset and `fee_address`, and creates the setup's mandate through `create`, signed by the principal;
- signs every call with the real key of its signer (`signed_by`, or the caller) over the exact invocation, so a call signed by the wrong actor fails host authorization as it would on a network; views carry no authorization;
- lifts the test host's own invocation limits, because the authorization snapshot the test host records for a large batch can exhaust them after the contract has finished, and instead checks every call against the per-transaction network limits (instructions, memory, disk reads, writes, footprint and event bytes), recording a call over any of them as a `host_error` that starts with `Budget.ExceededLimit`;
- passes a tagged destination as the muxed `M` address of the account and tag;
- makes attestations with the version 2 message and the tamper kinds of the vector format, with the label `#name` naming the test key whose seed is the label's bytes;
- applies `set_receiving` by writing the account's trustline (removing one rebuilds the environment without it, since the test host has no call that deletes an entry), and `clawback` through the asset's `clawback`.

A contract error code that another contract raised and the mandate let escape, such as a Stellar Asset Contract error, is recorded as that contract's (`escaped_error`), never as the mandate error with the same number: the asset's codes 10 to 12 are also `UNAUTHORIZED`, `ROLE_CONFLICT` and `ROLE_SET_FULL`.

## Contract interface the driver expects

Every argument is built from this description, so the runner checks the contracts' interface instead of sharing their Rust types. A struct is a `#[contracttype]` struct (a map keyed by field name), an optional value is `Option`, and every 32-byte value is `BytesN<32>`.

Factory:

- `__constructor(mandate_wasm_hash: BytesN<32>, assets: Vec<Address>, fee_address: Address)`
- `create(config: MandateConfig, salt: BytesN<32>) -> Result<Address, MandateError>`, requiring `config.principal`'s authorization.

Mandate, deployed by the factory:

- `__constructor(factory: Address, fee_address: Address, config: MandateConfig) -> Result<(), MandateError>`, called by the factory's deploy.
- `MandateConfig { principal, asset, recipient_mode: u32, tenant_ref, agents, approvers, approvals_required: u32, guardians, executors, recovery, recovery_threshold: u32, limits: Limits, attester: Option<Bytes>, country_policy_hash, max_fee_bps: u32 }`
- `Limits { total_budget: i128, per_payment_cap: i128, per_recipient_daily_cap: i128, approval_threshold: i128, payout_ttl_seconds: u64, expires_at: u64 }`
- `PayoutRequest { payout_id, recipient_ref, total: i128, fee: i128, legs: Vec<Leg>, deadline: u64, task_ref, attestations: Vec<Attestation> }`
- `Leg { destination: MuxedAddress, amount: i128 }`, `Attestation { valid_until: u64, policy_epoch: u64, signature: Bytes }`, `AllowEntry { recipient_ref, destination: MuxedAddress }`
- Functions with the names and argument order of the vector format's call table. The functions that take a caller take it first: `request_payout`, `request_payouts`, `approve`, `reject`, `cancel`, `cancel_batch`, `execute`, `execute_batch`, `pause`, `sweep`, `recovery_revoke`, `recovery_rotate_agent` (whose `new` is `Option<Address>`). `set_attestation_policy` takes the attester as `Bytes`. Errors are `MandateError` from `soroban/crates/mandate-errors`.
- Returns: a status as `u32` (`1 PENDING` to `5 CANCELLED`); `request_payouts` and `cancel_batch` a list of `ItemResult { payout_id, code: u32, status: Option<u32> }`; `revoke` and `sweep` an `i128`; the recovery calls a `u32`.
- Views: `state()` returns `{ spent, pending, committed, balance, free, paused, revoked, expired, policy_epoch: u64 }`; `config()` returns the configuration plus `fee_address`, `proposed_principal: Option<Address>`, `recovery_epoch` and `agent_epoch`; `payout(id)` returns the record, the tombstone `{ payout_id, status }` (a `#[contracttype]` enum wrapping either is accepted) or nothing; `daily_used(recipient_ref)` returns `{ day: u64, used: i128 }`; `is_allowed` a `bool`; `version()` a `u32`.
- Events: two topics, the event name and the key of `spec/EVENTS.md`, with the data a map of the remaining fields. Recipient modes, statuses and item codes are written as their numbers; the trace writes their names.

## The trace

`tests/trace.schema.json` is the trace's schema. A trace holds the chain, the SHA-256 of each contract wasm, and one entry per selected vector:

- `status`: `ran`, `aborted` (with `reason`, the steps so far kept), `not_runnable` (the wasm is not built) or `skipped` (the vector does not apply to the chain).
- `actors`: the address each name stood for.
- `setup_state` and, for each step, `state`: the mandate's `state()` and the fields of `config()` a vector's state can name, every payout and recipient the vector names, and each holder's balance of both tokens; null before the mandate exists.
- each step as the vector wrote it, without `expect`, plus `index` (from 1), `now`, and for a call its `result`, its `events` (the mandate's and the factory's, in order, in the canonical form, with the topic key restored as a field), and its `token_events`.

A `result` is `{ ok, return }`, `{ error }` with an error name, `{ host_auth_failure }`, `{ host_abort }` (a host function trap, such as `Crypto.InvalidInput`), `{ host_error }`, `{ host_panic }` or `{ escaped_error }`. Values are in the vector format's written forms: actor names, labels where the vector used one and hex otherwise, decimal strings for amounts and times.

## The comparator

For each call it checks the result against `expect` (a `per_chain` override for the chain replaces it), the events exhaustively and in order, `token_events: []` when given, and the named parts of `state` only. Labels, hex and attester keys are compared by the bytes they stand for. After every step it checks invariants I1 to I10 and I13 of `spec/MANDATE.md` on the recorded state, events and results; a failed call that emitted anything or changed the state fails I9.

Lists are compared in order, role sets included. The vectors write a role set in insertion order with removed members taken out and the rest keeping their order (`["agent2", "agent3"]` after `agent1` is rotated out and `agent3` added), so the mandate's `config()` and its events must report the set in the order it stores it, which must be that order: append on add, remove without reordering. Comparing two traces checks the results, events, states and whether tokens moved, step by step.

## Self tests

`cargo test -p vector-runner` in `soroban/` proves the runner without the real mandate: comparator cases built from the shared vectors (`extra_event_fails`, `missing_event_fails`, `reordered_events_fail`, `invariant_violation_fails`, `only_calls_selects_matching_vectors`), runs against the test-only reference factory and mandate in `soroban/vector-runner/contracts` (`reference_contract_wrong_return_is_reported`, `token_error_is_never_read_as_a_mandate_error`, `runner_registers_wasm_only`), the token kinds, receiving states and clawback on the test host, and the attest step against every case of `spikes/attestation-parity/fixture.json`. The reference contracts are committed as wasm in `soroban/vector-runner/fixtures`, and `scripts/check-wasm-fixtures.sh` checks they match their source.

The four Stellar only cases in `soroban/vector-runner/chain-specific/stellar.json` need the real contracts, built first with `scripts/soroban-build.sh`, and run with the rest of the self tests: `cargo test -p vector-runner chain_specific`.
