# Internal audit: Soroban mandate and factory

The low findings are tracked in #132 (L1), #133 (L2), #134 (L3) and #135 (L4).

* Commit audited: `691f19bd979f819284d59bc68c41ff37f02c3aba` (`development`, 691f19b).
* Scope:
  * `soroban/contracts/mandate/src`: lib, types, storage, derived, events, payout, execute, attest, lifecycle, admin, recovery.
  * `soroban/contracts/factory/src/lib.rs`.
  * The shared crates `soroban/crates/mandate-config` and `soroban/crates/mandate-errors`.
  * Read against `spec/MANDATE.md`, `spec/EVENTS.md`, `spec/ENCODING.md`, `spec/errors.json` and `spec/constants.json`.
* Method: I read every line of the production contracts and both shared crates. I compared each entry point's check order with its numbered list in MANDATE.md, ran the static analyzers myself, and read the exploit suite and its CI results. I did not take `audits/internal-exploit-pass.md` on trust.
* Contract source compared with the exploit pass and the testnet deploy:
  * The exploit pass and the testnet deployment were made at `e306910`.
  * Between `e306910` and `691f19b` the only changes are a property test file, a proptest dev dependency and Cargo.lock entries for it. No contract source changed.
  * CI on 691f19b (run 41246309952) built factory wasm `a19ae223...3a36` and mandate wasm `be92ba13...6ca0`. These equal `factory_wasm_hash` and `mandate_wasm_hash` in `deployments/testnet.json`, so testnet runs exactly this code.

## Summary

| Severity | Count | Issues filed |
| --- | --- | --- |
| Critical | 0 | none |
| High | 0 | none |
| Medium | 0 | none |
| Low | 4 | not filed (below medium) |
| Informational | 8 | not filed |

Every finding is open until it is fixed and re-tested, or accepted with the maintainer's recorded approval.

## Tool and suite evidence

**cargo clippy** (local, toolchain 1.98.1, offline, own target dir):
* Command: `cargo clippy --locked -p ujira-mandate -p ujira-factory -p mandate-config -p mandate-errors --all-targets -- -D warnings`.
* Result: finished with zero warnings.
* CI ran the same with `--workspace` on 691f19b and passed.

**cargo clippy with extra audit lints** (lib targets only):
* Lints: `arithmetic_side_effects`, `indexing_slicing`, `unwrap_used`, `expect_used`, `panic`, `as_conversions`, `cast_possible_truncation`, `integer_division`.
* Result: 4 warnings, all benign:
  * `payout.rs:347`: `i + 1` on a loop index of at most 4.
  * `derived.rs:55`: `now / 86400`, which is the specified day number.
  * `payout.rs:183` and `payout.rs:547`: `e as u32` on a `#[repr(u32)]` error enum.
* There is no unchecked amount arithmetic, no `unwrap`, `expect`, `panic` or indexing in any contract or shared crate.

**cargo deny 0.20.2** (the CI binary, sha256 checked, run locally):
* `check advisories licenses sources`: "advisories ok, licenses ok, sources ok".
* `check bans`: "bans ok". There are duplicate version warnings only, which are not policy failures.

**cargo audit 0.22.2** (release binary):
* It could not fetch the advisory database itself (timeout), so I ran it against the RustSec database that cargo deny had fetched (git head `2295466c`, dated 2026-10-30), with `--no-fetch`.
* Result: 1 vulnerability, RUSTSEC-2026-0387 in crypto-bigint 0.5.5, and 1 allowed warning, RUSTSEC-2024-0436 (paste, unmaintained).
* `cargo tree -p ujira-mandate` and `-p ujira-factory` for target `wasm32v1-none` find neither crate. Both are test host or compile time only, which matches the deny.toml ignore reasons. See L4.

**Exploit suite**, CI run 41246309952 on 691f19b:
* `ujira-exploits`: 70 passed, 0 failed.
* `ujira-mandate` unit tests: 179 passed.
* `ujira-factory`: 22 passed.

**Shared vectors**, CI `vectors-stellar` on 691f19b:
* "585 selected: 543 passed, 0 failed, 0 not runnable, 42 skipped for stellar".
* The runner loads the production wasm built in the same job.
* Every skip in the vector files has the skip reason `unrepresentable_input`.

**Other CI jobs** on 691f19b (error-codes, fixtures, headers including the spec ID and check order check, secrets, dashes): all success.

## Findings

### L1 (low): legs and allowlist entries may name addresses that can never pay anyone out

**Location:**
* `soroban/contracts/mandate/src/payout.rs:330-365` (`check_legs`).
* `soroban/contracts/mandate/src/admin.rs:505-520` (`check_entries`).
* Contrast with `soroban/contracts/factory/src/lib.rs:218-226` (`is_zero_address`).

**What I found:**
* Only the fee address is refused as a leg destination or allowlist entry.
* The mandate also accepts these as destinations:
  * its own address;
  * the settlement asset's contract address;
  * the account and the contract with an all-zero key.
* The spec says the zero address rules (MANDATE-SPLIT-003, MANDATE-RECIPIENT-016, MANDATE-ROLE-017) "can only be reached on Celo" because "Stellar has no zero address". The factory in the same repository says the opposite, that these all-zero addresses are valid addresses nobody controls, and refuses them as a fee address. Those spec rules therefore have Stellar inputs that the vectors skip as unrepresentable.

**Scenario:**
* Through a principal mistake in ALLOWLIST mode, or a wrong attestation in ATTESTED mode, a leg to the all-zero contract address or to the asset contract executes normally.
* The settlement asset contract accepts transfers to contract addresses, so the leg amount is burned. I reasoned this from how the asset contract handles contract balances; I did not reproduce it on testnet.
* A leg to the mandate itself spends budget and pays the fee each time while the principal's money goes nowhere. Up to 1 percent per cycle leaks to the fee address.
* Every case needs a party that binds destinations (the principal or the attester) to bind a bad address. No agent can do it alone.

**Fix:**
* In `check_legs`, `check_entries`, and in the off-chain attester, refuse:
  * `env.current_contract_address()`;
  * `fixed.asset`;
  * all-zero-key addresses.
* Reuse the factory's `is_zero_address` for the last check.
* Change SPLIT-003 and RECIPIENT-016 so they apply on Stellar with these addresses, and add Stellar vectors for them.

**Covered by vectors or exploits:** no. The zero address vectors are skipped for Stellar, and exploit 18 covers only the fee address.

### L2 (low): an approver who is removed and added back gets their old approvals back

**Location:**
* `soroban/contracts/mandate/src/payout.rs:417-424` (`live_approvals`).
* `soroban/contracts/mandate/src/payout.rs:446-458`.

**What I found:** the live count is the number of entries in `record.approvals` that are current approvers. Approvers carry no epoch, unlike agents (MANDATE-PAYOUT-014).

**Scenario:**
1. Approver A approves PENDING payout P with `approvals_required = 2`.
2. A's key is suspected, so the principal removes A.
3. Later the principal adds the same address back.
4. A's old approval of P counts again without A acting. One more approval completes P.

The principal has to re-add the same address for this to happen, so the impact is limited.

**Fix (either):**
* Give approvers an epoch, as agents have, and count only approvals made under the approver's current epoch.
* Or remove an approver's entries from open records when it is removed. This is unbounded, so the epoch is preferable.

Otherwise, record in MANDATE-APPROVAL-004 that this is accepted.

**Covered by vectors or exploits:** no. `replay_approval_from_removed_approver_does_not_count` (exploits/src/replay.rs:286) covers removal, but not adding the same address back.

### L3 (low): removing a compromised agent does not release what it reserved

**Location:**
* `soroban/contracts/mandate/src/payout.rs:263-271` (reservation at request).
* `soroban/contracts/mandate/src/admin.rs:401-413` (agent removal).
* `soroban/contracts/mandate/src/payout.rs:589-608` (who may cancel).

**What I found:**
* Nothing limits how many payouts can be open at once.
* After the agent is removed, its payouts cannot execute (AGENT_REMOVED), but their `pending` and `committed` amounts stay reserved.
* The reservation is released only by one cancel per payout (at most 10 per transaction) or by a revoke.

**Scenario:**
1. A leaked agent key queues thousands of 1-unit APPROVED payouts until `free` is 0. The agent pays the fees and rent for this.
2. While the mandate is active, `withdraw` is capped at `free`, which is now 0, and new requests fail with INSUFFICIENT_FUNDS.
3. The principal must then do one of these:
   * cancel every payout;
   * revoke, which is final and means creating a new mandate;
   * shorten `expires_at` with `update_limits` and withdraw.

   If it takes the third route, reactivating later needs `balance >= pending + committed` (MANDATE-FUND-004).

No funds are lost; this costs liveness and fees.

**Fix (any of):**
* A per-agent or per-mandate limit on open payouts.
* Or have `remove_role(AGENT)` and `recovery_rotate_agent` record that the agent's queue is dead, so the reservations can be released in one call.
* At minimum, document the incident runbook (revoke, withdraw, new mandate) under CHAINOPS.

**Covered by vectors or exploits:** partly. `lifecycle_stranger_cannot_unwind_live_payouts` and the budget model tests cover the related flows, but not mass reservation by a removed agent.

### L4 (low): cargo audit reports a vulnerability that CI does not run cargo audit for

**Location:**
* `soroban/Cargo.lock` (crypto-bigint 0.5.5).
* `soroban/deny.toml` ignore list.
* `.github/workflows/ci.yml` (cargo deny only).

**What I found:**
* `cargo audit` exits with "1 vulnerability found" (RUSTSEC-2026-0387).
* cargo deny ignores it, with a reason I confirmed: the crate is not in the `wasm32v1-none` graph of either contract.
* The brief asks for cargo audit, but CI runs only cargo deny. A future RustSec entry that cargo deny's config ignores, or a database that cargo deny cannot fetch, would go unnoticed.

**Fix:**
* Either add a pinned `cargo audit` step to CI, with matching `--ignore` entries that carry the same reasons as deny.toml.
* Or record in `audits/README.md` that cargo deny's advisories check is the gate that replaces cargo audit.
* Remove both ignores once soroban-env-host moves past elliptic-curve 0.13.

**Covered by vectors or exploits:** not applicable.

### I1 (informational): the mandate expiry branch of cancel reason 3 can never decide anything

**Location:** `soroban/contracts/mandate/src/payout.rs:600-606`.

**Why it is unreachable:**
* A payout's `expires_at <= created_at + PAYOUT_TTL_MAX`, which is 30 days.
* The mandate's `expires_at` is always greater than `now` when it is set, so it is later than every existing payout's `created_at`.
* So `now >= mandate expires_at + SWEEP_GRACE_SECONDS` (also 30 days) implies `now >= payout.expires_at`, and the first branch has already returned.

This is harmless. Keep the branch for spec parity, or note in MANDATE-PAYOUT-010 that it is a backstop.

### I2 (informational): changes to recovery_epoch and agent epochs are not evented directly

**Location:**
* `admin.rs:312-315` and `admin.rs:401-413` (agent add and remove bump `recovery_epoch`).
* `recovery.rs:222-229` (rotation pushes a new agent epoch).

**What I found:** `role_added` and `role_removed` carry neither the new `recovery_epoch` nor the agent's epoch. An indexer can only learn them by reading `config()` or from the next `recovery_vote_added`. The spec event table matches the code, so this is not a conformance failure.

**Fix:** consider these fields for spec version 2 (alongside #98 and #99).

### I3 (informational): the factory never extends the mandate code entry it deploys from

**Location:** `soroban/contracts/factory/src/lib.rs:112-155` and `212-216`.

**What I found:** `create` extends only the factory instance. The mandate wasm stays live only while some mandate is called. If no mandate is used for long enough, the next `create` needs a restore. With auto restore this costs only fees, and the simulation handles it.

**Fix:** call `extend_ttl` on the mandate wasm code in `create`, or keep it in the operator's TTL runbook (CHAINOPS-SUBMIT-002).

### I4 (informational): the exploit suite simulates archival, it does not exercise real eviction

**Location:** `exploits/src/storage_events.rs:18-24`, and `replay.rs:71-81`.

**What I found:** "archival" is modelled by advancing the ledger sequence in the test host. That shows the stored values survive; it does not prove behaviour after eviction on the network. That rests on protocol semantics, as MANDATE-PAYOUT-009 and MANDATE-PAYOUT-016 say.

**Fix:** before mainnet, add a testnet check that restores an evicted tombstone and an evicted allowlist entry.

### I5 (informational): batch resource bounds were measured against testnet settings

**Location:**
* `MAX_BATCH_TRANSFERS` at `soroban/contracts/mandate/src/execute.rs:34`.
* `soroban/spikes/batch-limits` (3 tests passed in CI).

**What I found:** the "under 70 percent of every network limit" claim depends on network config settings: the events size limit, read and write entry limits, and the instruction limit. I did not re-measure them.

**Fix:** re-run the batch-limits spike against mainnet config settings before the mainnet deploy.

**Follow-up, after this audit:** `soroban/batch-limits` now runs the worst cases on the production wasm in CI, against the settings read from testnet and mainnet on the date its output names. Re-read them before the mainnet deploy.

### I6 (informational): after a revoke, payout records still read as open

**Location:** `soroban/contracts/mandate/src/lifecycle.rs:132-154`, and `lib.rs:286-289`.

**What I found:** revoke sets `pending` and `committed` to 0 but leaves the records PENDING or APPROVED. `cancel` and `reject` are refused once revoked, so `payout(id)` reports them as open forever. Nothing can act on them, so this is not a safety problem.

**Fix:** off-chain readers must treat any record on a revoked mandate as dead. Document this beside MANDATE-REVOKE-001.

### I7 (informational): a lost principal key means funds that cannot be recovered, by design

**Location:** `lifecycle.rs:188-223`, `recovery.rs`.

**What I found:** recovery can revoke but never change the principal. `sweep` only ever pays the principal. If the principal's key is lost, the balance is unreachable.

This is the intended non-custodial design (MANDATE-RECOVERYSET-009, MANDATE-SWEEP-005). CHAINOPS-CREATE-002 already requires multisig for large budgets; keep that.

### I8 (informational): the placeholder contract builds by default

**Location:** `soroban/Cargo.toml` (`default-members` includes `contracts/placeholder`).

**What I found:** it exposes only `version()` and is not in `deployments/testnet.json`.

**Fix:** make sure the mainnet deploy script cannot pick it up.

## Known open items, assessed and not re-reported

* **#123** (a mandate cannot prove which factory deployed it). Confirmed:
  * The constructor stores `factory` unauthenticated (`lib.rs:66-83`).
  * Anyone can deploy the mandate wasm directly with any `fee_address` and `max_fee_bps` up to 100.
  * The only defences are off chain: recompute the address, or require the factory's `mandate_created`.
  * This should stay a blocker for any integration that discovers mandates without the factory.
  * Decision, made after this audit: no on-chain proof. The constructor cannot authenticate its deployer, so a stored or exposed factory address would still be a claim, and a registry in the factory would add storage and rent to every create to answer a question the address already answers. The defence stays off chain, in `CHAINOPS-INDEX-001`, `CHAINOPS-INDEX-002` and `CHAINOPS-INDEX-005`.
  * Exploit 36 now models a reader following those rules and shows it refusing the strongest look-alike: the factory's own code, its fee address and a supported asset, for a principal who signed nothing.
  * Still open: the suite proves the rule, not the production reader. The decision holds only once the chain adapter enforces `CHAINOPS-INDEX-005`, and that must be confirmed in the security review before go-live. This acceptance needs the maintainer's recorded approval, like any other finding.
* **#99** (event binding and `tenant_ref`). Confirmed:
  * `tenant_ref` is supplied by the principal and is the topic key of every event that is not a payout event.
  * Look-alike contracts can emit identical events, so intake must bind the contract ID.
* **#111** (expiry overflow and `daily_used` on a stale day). Confirmed:
  * `payout.rs:272` returns INTERNAL_INVARIANT when `now + payout_ttl_seconds` overflows u64. This is reachable only with ledger times near u64 max.
  * `lib.rs:293-303` returns `{ today, 0 }` for a stale stored day.
  * There is a builder worktree for #111 at `worktrees/ujira-contracts-111`.
* **#98** (data for topic-only events). Confirmed: `payout_approved` has an empty data map (`events.rs:73-78`), which matches EVENTS.md.

## Checklist

**1. Authorization and role separation: PASS.**
* Every entry point that takes a caller calls `caller.require_auth()` first:
  * request_payout and request_payouts (payout.rs:153, 168);
  * approve (427), reject (490), cancel (520), cancel_batch (532);
  * execute and execute_batch (execute.rs:50, 69);
  * pause (lifecycle.rs:79), sweep (189);
  * recovery calls (recovery.rs:76).
* Every principal-only call goes through `authorized_principal` (lifecycle.rs:72-76), which reads the stored principal and requires its auth.
* `accept_principal` requires the proposed address's auth after checking that a proposal exists (admin.rs:670-671).
* `create` requires `config.principal` auth (factory lib.rs:117).
* The transaction source and fee payer are never read.
* Role separation is enforced:
  * at creation (mandate-config lib.rs:181-203);
  * on add (admin.rs:216-243, 296-301);
  * on rotation (recovery.rs:200-214);
  * on propose and accept (admin.rs:650-677).

  The overlaps it allows are exactly those in MANDATE-ROLE-006.
* The fee address is refused for every role and as the principal.
* The attester key check covers every path that ROLE-018 lists.
* Exploit attempts 1 to 7 cover this.

**2. Check order: PASS.** I compared each operation line by line with its spec list and all match:
* request_payout 1-28 (payout.rs:153-261, attest.rs:51-84);
* request_payouts, approve, reject, cancel and cancel_batch;
* execute 1-15 and execute_batch (execute.rs:49-167);
* pause, resume, revoke, withdraw, sweep and recover_token;
* update_limits 1-6;
* add_role, remove_role, set_approvals_required and set_recovery_threshold;
* allow and disallow;
* set_attestation_policy, propose, accept, cancel proposal and lower_fee_cap;
* recovery_revoke and recovery_rotate_agent;
* factory create 1-16, through `mandate_config::validate`.

The two zero address steps that are marked Celo only have no Stellar check (see L1).

**3. Arithmetic: PASS.**
* All amount arithmetic is checked: `check_amounts`, `check_legs`, the budget and headroom forms of MANDATE-AMOUNT-005, `release`, `pay`, and `fee_cap` split as the spec defines it.
* Epoch counters use `checked_add`.
* TTL math saturates, and `attest.rs` uses `checked_sub` after the expiry check.
* The release profile sets `overflow-checks = true`.
* The extra clippy lint pass found no unchecked arithmetic.
* Exploits 9, 23 and 24 and the fee cap property tests (over 100,000 cases each) cover this.
* The only open overflow is #111.

**4. Storage and archival: PASS, with I3 and I4.**
* Rule state is only in instance or persistent storage. Temporary storage is never used.
* Tombstones replace records at the same key and keep its TTL (storage.rs:293-301).
* Record TTL follows MANDATE-PAYOUT-016 (storage.rs:259-280).
* Every entry point extends the instance (and so the code).
* Recovery votes use a distinct key enum (recovery.rs:44-48), so they cannot collide with other keys.
* All role sets are bounded, so the instance entry stays small.

**5. Transfers and token handling: PASS.**
* Every transfer out is wrapped:
  * legs and fee by `try_invoke_contract`, where any error or non-unit return becomes TRANSFER_FAILED (execute.rs:220-231);
  * withdraw, sweep and recover_token by `try_transfer` (lifecycle.rs:257-268).
* Token error codes never pass through.
* State and the event are written before the first transfer in execute (execute.rs:179-214), and an Err return reverts the whole frame.
* The fee is paid last, only in execute.
* `recover_token` refuses the settlement asset and re-checks its balance (lifecycle.rs:230-251).
* withdraw and sweep take no destination.
* Exploits 25, 27, 28, 29 and 30 cover this.

**6. Reentrancy and external call safety: PASS.**
* The Soroban host refuses any call into a contract already on the call stack, and execute writes its effects before any transfer.
* The only external calls are to the fixed settlement asset and, in `recover_token`, to a token the principal chooses. Even there, the balance check guards the settlement asset.
* Exploit 27 covers execute, withdraw, sweep and views.

**7. Denial of service and fund locking: PASS, with L3, I6 and I7.**
* Batches, legs, role sets, the allowlist batch and the transfer budget are all bounded.
* revoke moves no funds.
* A principal that cannot receive can move to a new address and retry.
* A fee address that cannot receive affects liveness only (CHAINOPS-FEE-005, exploit 22).
* Pausing is bounded (exploit 34).
* A wrongly signed attestation trapping a whole batch is accepted by the spec (MANDATE-RECIPIENT-012).

**8. Upgradeability and admin powers: PASS.**
* Neither contract calls `update_current_contract_wasm` (I searched the source).
* The factory has no admin, setter or upgrade; its storage is written only by the constructor.
* The mandate's fee address is fixed, and the fee cap can only go down (admin.rs:707-722).
* The operator's powers are limited to its role keys (guardian, executor, agent, attester), each bounded as the spec says.

**9. Events: PASS for conformance, with I2.**
* All 29 events in EVENTS.md exist with the specified fields and topic keys (events.rs, factory lib.rs:53-63), in the specified order.
* Refused calls emit nothing (exploit 38).
* Binding and topic-only data are covered by the known items #99 and #98.

**10. Error codes: PASS.**
* The `mandate-errors` enum matches the spec table: 39 codes, 10 to 90.
* The CI `error-codes` job (check-error-codes.py against errors.json) passed on 691f19b.
* Every failure in a contract is a code; nothing traps, except the ed25519 verify that the spec requires to trap.

**11. Replay and idempotency: PASS.**
* Payout IDs are permanent through tombstones, and the request deadline makes a lost attempt decidable.
* Attestations bind the network, the mandate, the recipient, the destination and tag presence, and the policy epoch (attest.rs:116-142).
* Repeated approvals are refused, and recovery votes count once per epoch.
* Soroban auth nonces stop signature replay.
* create is deterministic and safe to retry after a failure.
* Exploits 13 to 16 and 37 cover this, with the gap noted in L2.

**12. Conformance with the shared spec and vectors: PASS, with L1.**
* 543 of 585 vectors pass on the production wasm, with 0 failures. The 42 skipped vectors are marked `unrepresentable_input`.
* The constants in code match `constants.json`.
* The attestation layout matches ENCODING.md, and CI checks the worked example.
* L1 is a gap where the spec calls Stellar inputs unrepresentable when they are not.

**13. Static analyzers: PASS for clippy and cargo deny, FAIL for cargo audit as raw output (L4).**

**14. Exploit suite: PASS.**
* 70 of 70 passed in CI on 691f19b.
* I checked the attempts against the code, not just their table. Exploit 36 and 39 rest on #123 and #99. Archival modelling is limited, as noted in I4.

<!-- ujira-review reviewer=contract-audit-reviewer sha=691f19bd979f819284d59bc68c41ff37f02c3aba verdict=approve -->
