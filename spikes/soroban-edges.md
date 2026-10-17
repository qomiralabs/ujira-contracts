# Soroban edges: transfer failure, constructor errors, archived entries

Three host behaviors that decide how the mandate's Soroban rules are worded. The tests live in `soroban/spikes/soroban-edges/`. Every finding the SDK test env can reproduce is pinned by a named test there, so an SDK bump that changes it fails CI. A few findings only show on a real network, because the test env behaves differently on purpose (it restores archived entries itself, it does not run RPC simulation, and native test contracts report host errors differently from wasm). Those are marked **network only** below, with the transactions that showed them, and have to be rechecked by hand on a protocol or RPC upgrade.

## Versions tested

| Component | Version |
| --- | --- |
| soroban-sdk, soroban-env-host | 29.0.1 |
| Rust | 1.98.1, target `wasm32v1-none` |
| stellar-cli | 28.1.0 |
| Local network | `stellar/quickstart` with stellar-core 29.0.0 and stellar-rpc 29.0.1, protocol 29 |
| Public network | Stellar testnet, protocol 29, ledgers 5243600 to 5243650 (2026-10-17) |

The local network had its state archival settings lowered (`min_persistent_ttl` 32, `min_temporary_ttl` 16, `starting_eviction_scan_level` 1) so entries could expire and be evicted within minutes. Testnet's `min_persistent_ttl` is 120,960 ledgers (about 7 days), so archival was measured locally only. Everything else was run on both networks.

## Layout and how to run

All paths are under `soroban/spikes/`.

- `soroban-edges/src/lib.rs`: the probe contracts. `Payer` changes its own state and then pays through `try_invoke_contract`, either returning `TRANSFER_FAILED` (execute shape) or carrying on (revoke shape). `Factory` deploys a child with or without checking its arguments first. `Store` writes and reads persistent and temporary records.
- `soroban-edges/child/`: a contract whose constructor fails on request. It doubles as a hostile token: with a stored value of 0 its `transfer` grows a vector until the budget runs out, and with any other value it returns that value.
- `soroban-edges/fixtures/soroban_edges_child.wasm`: the child built with `stellar contract build --package soroban-edges-child`. The deployer needs real wasm, so the build output is committed (sha256 `4c3c4f7aec653e12b3a74d21bbef044e5162869763f80b506f27b10a591cbec1`). `scripts/check-wasm-fixtures.sh` rebuilds it from source and fails if the bytes differ; rebuild and copy it whenever the child changes.

The spike crates are workspace members but not default members, so a plain `stellar contract build` or `cargo build` in `soroban/` builds only the production contracts and never writes the hostile child into the release folder. The gates run them with `--workspace`.

```
cd soroban
cargo test -p soroban-edges
cd .. && scripts/check-wasm-fixtures.sh
```

## 1. Transfer failure mapping

Tests: `transfer_failure_no_trustline`, `transfer_failure_deauthorized`, `transfer_failure_insufficient_balance`, `transfer_failure_rolls_back_earlier_legs`, `transfer_failure_rolled_back_write_is_still_charged`, `transfer_failure_budget_exhaustion_is_not_caught`, `transfer_failure_host_error_is_narrowed`, `transfer_returning_a_value_is_a_failure`.

Every SAC failure is caught by `try_invoke_contract` and arrives as a contract error the caller can map:

| Case | SAC error | Caught |
| --- | --- | --- |
| Recipient account exists, no trustline | `Error(Contract, #13)` "trustline entry is missing for account" | yes |
| Recipient account does not exist | `Error(Contract, #13)`, same as above | yes |
| Recipient trustline not authorized | `Error(Contract, #11)` "balance is deauthorized" | yes |
| Sender (the contract) deauthorized by the issuer | `Error(Contract, #11)` "balance is deauthorized" | yes |
| Amount above the balance | `Error(Contract, #10)` "balance is not sufficient to spend" | yes |

When the contract then returns its own error (`#72` here), the whole invocation is rolled back: the contract's own earlier write, its event, and any leg that had already been paid in the same call. Pinned in the test env, and also seen on a submitted transaction that failed at apply time (local, tx `ee5472ea...`, ledger 1155, result `trapped`, first leg of 300 undone). When the contract catches the failure and returns success instead, its own writes and events stay and no money moves.

What `try_invoke_contract` does not catch: budget exhaustion, storage limit errors (`ExceededLimit` of type Budget or Storage) and host internal errors (from the host source). A token that burns the budget fails the whole call: in `transfer_failure_budget_exhaustion_is_not_caught` the child, running as wasm, exhausts the budget inside `transfer`, `try_call` hands `Error(Budget(ExceededLimit))` back instead of a value, the calling frame can only escalate it, and the caller's write is gone. The same call simulated against a wasm caller on the local network and on testnet fails with `Error(Budget, ExceededLimit)`. Storage limit and internal errors are not tested; that part rests on the host source alone.

Any other recoverable host error reaches the caller narrowed to `Error(Context, InvalidAction)`, and the real cause is only in the diagnostic log. `transfer_failure_host_error_is_narrowed` pins it by calling `transfer` on an address with no contract. A token whose `transfer` returns a value instead of nothing is a failure too (`transfer_returning_a_value_is_a_failure`). So the mapping has to accept any error and any unexpected return, not a list of SAC codes.

Resource accounting: when the failure is on the recipient side (no account, no trustline, unauthorized trustline), the SAC has already written the sender's balance before it fails. That write is rolled back, but it still counts against the transaction's write bytes. stellar-rpc 29.0.1 simulation leaves it out, so a call that catches such a failure and succeeds is under-declared and fails on chain with `InvokeHostFunctionResourceLimitExceeded`:

| Network | Declared by simulation | Needed at apply |
| --- | --- | --- |
| Local | 104 bytes | 328 bytes |
| Testnet (tx `ce94d6ba...`, ledger 5243610) | 104 bytes | 224 bytes |

The same call with the write bytes padded by 400 lands and returns the caught `#13` (local, unpadded tx `c9898ba4...` failed, padded tx `052efd01...` succeeded; testnet `9791d557...` failed, padded `e0ecead5...` succeeded). Sender-side failures (insufficient balance, deauthorized sender) fail before any write and land unpadded. The test env charges the rolled-back write (328 bytes, 2 entries), and `transfer_failure_rolled_back_write_is_still_charged` pins that. The simulation gap itself is **network only**: the test env runs no RPC simulation.

Implied rule wording:

- Every transfer out of the mandate (legs, fee, withdraw, sweep, revoke, recover) goes through `try_invoke_contract`. Any error the call returns, whatever its type, is `TRANSFER_FAILED`. Where the rule says the call fails with `TRANSFER_FAILED`, the function returns that error and the host reverts every effect of the call, earlier legs included.
- A revoke whose return transfer fails still succeeds and keeps its state changes. The submitter must declare write bytes for the SAC balance entry the failed transfer touched, because simulation does not.
- Budget exhaustion inside a token call is a transport failure, not `TRANSFER_FAILED`. Only `recover_token` calls a token we did not pick, so only it can be failed this way by a hostile token.

## 2. Constructor errors through a deployer

Tests: `constructor_error_through_deployer`, `constructor_error_checked_in_factory`, `constructor_error_code_is_only_in_diagnostics`.

A constructor that returns `Error(Contract, #34)` reaches the factory's caller as `Error(Context, InvalidAction)`. The host rewrites any recoverable constructor error on purpose, so that a constructor cannot leak codes into the contract that deployed it. The original `#34` survives only in diagnostic events ("constructor invocation has failed with error"), which are not part of the transaction result.

Nothing is left behind: no instance entry at the deterministic address, no contract events, and the same salt deploys cleanly once the arguments are good. Pinned in the test env, and also seen on submitted transactions, where the meta held only the fee events (local tx `48564536...`, ledger 1255; testnet tx `27890fa7...`, ledger 5243638; both `trapped`, then a clean retry at the predicted address).

A factory that checks the arguments itself before deploying returns its own `#34` to the caller.

Side finding, **network only** for the wasm half: a native test contract and the same contract running as wasm report host errors differently. A second deploy at a used address shows `Error(Context, InvalidAction)` through the native test factory and `Error(Storage, ExistingValue)` on the network. Contract error codes agree.

Implied rule wording:

- On Soroban the factory runs the full config validation, the same function the constructor uses, before it deploys, and returns `CONFIG_INVALID`, `ROLE_CONFLICT` or `ROLE_SET_FULL` itself. The constructor keeps the same checks as a backstop; if one fires through the factory, the caller sees a host error and that is a factory bug.
- A failed create leaves no mandate, so a retry with the same salt is safe.
- Vector runs on Soroban use the compiled wasm, not native registration, so that any host error a vector sees is the one a chain reports.

## 3. Archived entries

Tests: `archived_entry_read`, `temporary_entry_lapses_to_absent`.

A persistent entry whose TTL has run out is archived, never deleted, and never reads as absent:

- Read in a transaction built from simulation: simulation marks the archived entries for automatic restore (`archived_soroban_entries` lists the record, the contract instance and the wasm code when all three had lapsed), the read returns the stored value, and `has` returns `true`. Local tx `9d27b500...`, ledger 940.
- Read in a transaction without the restore marking: the transaction fails with `InvokeHostFunctionEntryArchived` ("trying to access an archived contract data entry") and changes nothing. Local tx `33543393...`, ledger 898.
- Evicted to the hot archive (gone from the live bucket list): exactly the same two outcomes. Restore tx `51dfc582...`, ledger 1106, returned `true` and the original value; unmarked tx `b951c198...`, ledger 1123, failed with `EntryArchived`.
- A restored entry lives for `min_persistent_ttl - 1` ledgers from the restoring ledger (4,095 in the test env, 31 on the local network).
- RPC `getLedgerEntries` returns an archived or evicted entry with `liveUntilLedgerSeq: 0`, and returns nothing for a key that was never written.

A temporary entry past its TTL is gone: `has` returns `false` and the entry is no longer in storage, on both the test env and the network.

The test env restores archived entries automatically, like a simulated transaction, so `archived_entry_read` pins the restore and the minimum TTL. The failing path without restore marking, eviction to the hot archive, and the RPC `liveUntilLedgerSeq: 0` reading are **network only** (local network, transactions above).

Implied rule wording:

- Payout records and tombstones are persistent entries. An archived ID is restored before it is read, so it returns `PAYOUT_ID_EXISTS` exactly like a live one; a transaction that does not restore it fails and changes nothing. An archived record can never make an ID look unused.
- Off-chain readers treat an entry with `liveUntilLedgerSeq` 0 as present and archived, never as missing, and extend TTLs after a restore because restored entries come back with the minimum TTL.
- No rule state in temporary storage: an expired temporary entry is indistinguishable from one never written.
