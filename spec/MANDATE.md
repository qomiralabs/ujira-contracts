# Mandate rules, spec version 1

A mandate is a contract that holds a principal's settlement asset and lets agents pay recipients only inside limits the principal sets. This document is the single, language-agnostic definition of those rules. The Soroban contract in `soroban/` implements it, and the shared test vectors are written against it. When a contract and this document disagree, the contract is wrong. If an implementation for another chain is added, it must pass the same vectors, and wherever two implementations would behave differently on the same input, this document is wrong and must be fixed first.

Status: draft for spec version 1. The machine-readable error table (`spec/errors.json`), the constants (`spec/constants.json`), the event encodings (`spec/EVENTS.md`) and the ID and attestation message byte layouts (`spec/ENCODING.md`) sit beside this document and must match it; `scripts/check-spec-ids.sh` checks that they do.

Scope: spec version 1 is in force on Stellar only. Rules and text marked Celo or EVM are kept for reference and are not in force. An EVM chain added later needs a spec revision that defines its attestation scheme (for example secp256k1 with `ecrecover` over the same SHA-256 digest) and its bad-signature behaviour, and must not run `ATTESTED` or `BOTH` mandates until that revision exists. Under ed25519, `MANDATE-ROLE-018` could never match on an EVM chain, because an EVM address is not an ed25519 key.

## How to read this document

- Every rule has a stable ID, `MANDATE-<AREA>-<NNN>` for rules the contracts enforce and `CHAINOPS-<AREA>-<NNN>` for obligations on the off-chain services. IDs are never reused or renumbered. Every rule is listed in the rule index at the end.
- A contract rule states its precondition, its effect and the error it returns. "Error: none" means the rule constrains behaviour without adding a failure of its own.
- Every operation has a numbered check order. The first failing check decides the error, and nothing is written or emitted on failure. Every numbered check cites exactly one rule.
- The shared prefix "active checks" means three checks in this order: revoked (`MANDATE-REVOKE-002`), expired (`MANDATE-EXPIRY-002`), paused (`MANDATE-PAUSE-001`).
- Names are canonical snake_case. The Celo contract uses the camelCase equivalent (`request_payout` is `requestPayout`). Where a type differs it is written `soroban / solidity`.
- `now` is the ledger timestamp on Stellar and the block timestamp on Celo, in unix seconds, and takes one value per transaction (`MANDATE-EXPIRY-001`).
- The words must, never and only are normative.

Rule areas: `CONFIG`, `ROLE`, `AMOUNT`, `FUND`, `ASSET`, `DUST`, `PAYOUT`, `BATCH`, `BUDGET`, `PAYMENT`, `DAILY`, `RECIPIENT`, `SPLIT`, `APPROVAL`, `EXPIRY`, `PAUSE`, `REVOKE`, `WITHDRAW`, `SWEEP`, `RECOVER` (token recovery), `RECOVERYSET` (the recovery approver set), `PRINCIPAL`, `EVENT`, `FEE`. Off-chain areas: `XCHAIN`, `DELIVERY`, `SUBMIT`, `CREATE`, `SWEEP`, `FEE`.

## Constants

The values in force on Stellar. A row marked Stellar only depends on Stellar storage or ledger timing; every other value is chain-neutral.

| Name | Value | Meaning |
| --- | --- | --- |
| `SPEC_VERSION` | 1 | Returned by `version()` |
| `I128_MAX` | 170141183460469231731687303715884105727 | 2^127 - 1, the largest amount |
| `SECONDS_PER_DAY` | 86400 | Length of a UTC day |
| `MAX_LEGS` | 4 | Legs per payout |
| `MAX_BATCH` | 10 | Items per `request_payouts`, `cancel_batch` and `execute_batch`, and entries per `allow_recipients` and `disallow_recipients` |
| `MAX_BATCH_TRANSFERS` | 18 | Token transfers per `execute_batch`: every leg of every item, plus one per item with a non-zero fee |
| `MAX_AGENTS` | 8 | Size bound of `agents` |
| `MAX_APPROVERS` | 10 | Size bound of `approvers` |
| `MAX_GUARDIANS` | 4 | Size bound of `guardians` |
| `MAX_EXECUTORS` | 4 | Size bound of `executors` |
| `MAX_RECOVERY` | 10 | Size bound of `recovery` |
| `MAX_ATTESTATION_VALIDITY` | 86400 | Longest window an attestation may be valid for |
| `ATTESTER_KEY_BYTES` | 32 | Length of the attester's ed25519 public key |
| `ATTESTATION_SIGNATURE_BYTES` | 64 | Length of an attestation's ed25519 signature |
| `ATTESTATION_MESSAGE_BYTES` | 47 | Length of the signed attestation message: the 15-byte prefix and a 32-byte SHA-256 (`spec/ENCODING.md`) |
| `MAX_FEE_BPS` | 100 | Ceiling on `max_fee_bps` (1 percent) |
| `BPS_DENOMINATOR` | 10000 | Basis points per whole |
| `PAYOUT_TTL_MIN` | 3600 | Shortest payout time to live (1 hour) |
| `PAYOUT_TTL_MAX` | 2592000 | Longest payout time to live (30 days) |
| `SWEEP_GRACE_SECONDS` | 2592000 | Time after expiry during which only the principal may sweep (30 days) |
| `RECORD_TTL_MARGIN_SECONDS` | 86400 | Stellar storage only: how long a payout record stays live past its own expiry (1 day); no effect on Celo |
| `STELLAR_LEDGER_SECONDS` | 5 | Stellar only: the ledger close time used to convert seconds into ledgers for a TTL; the contract cannot read the close time, and 5 matches the TTLs measured on testnet |

Why the batch bounds: on Stellar the binding limit is the per-transaction contract events limit (16,384 bytes, events and return value together). Every token transfer emits its own transfer event that the mandate cannot avoid, and a leg to a tagged destination costs the most, so `execute_batch` is bounded by transfers as well as by items. Under these bounds every worst case, including all-tagged destinations, stays under 70 percent of every network limit on both chains. With 18 transfers a batch of two-leg payouts stops at 9 items; `MAX_BATCH` binds only for single-leg payouts without a fee.

## Errors

Codes are append only and never reused. `0` means success and appears only in batch item results.

| Code | Name | Meaning |
| --- | --- | --- |
| 10 | `UNAUTHORIZED` | The caller does not hold a role this call allows |
| 11 | `ROLE_CONFLICT` | An address would hold two roles that must stay apart |
| 12 | `ROLE_SET_FULL` | A role set would exceed its bound |
| 20 | `MANDATE_REVOKED` | The mandate is revoked |
| 21 | `MANDATE_EXPIRED` | The mandate is expired |
| 22 | `MANDATE_PAUSED` | The mandate is paused |
| 23 | `ALREADY_PAUSED` | Pause while paused |
| 24 | `NOT_PAUSED` | Resume while not paused |
| 25 | `MANDATE_ACTIVE` | Sweep while the mandate is active |
| 26 | `PRINCIPAL_CHANGE_PENDING` | A third-party sweep while a principal change is proposed |
| 27 | `SWEEP_GRACE_PERIOD` | A third-party sweep within the grace after expiry |
| 30 | `AMOUNT_INVALID` | An amount outside its domain |
| 31 | `LEGS_INVALID` | Leg count, leg amount, zero address, tag form or duplicate destination |
| 32 | `LEGS_SUM_MISMATCH` | Legs do not sum to the total |
| 33 | `BATCH_INVALID` | Batch length or transfer budget out of bounds |
| 34 | `CONFIG_INVALID` | A configuration value or change is not allowed, including an attester key that is not 32 bytes or is 32 zero bytes |
| 35 | `REQUEST_DEADLINE_PASSED` | The request arrived at or after its deadline |
| 40 | `BUDGET_EXCEEDED` | The charge exceeds the remaining budget |
| 41 | `PAYMENT_CAP_EXCEEDED` | The charge exceeds the per-payment cap |
| 42 | `DAILY_CAP_EXCEEDED` | The total exceeds the recipient's daily headroom |
| 43 | `INSUFFICIENT_FUNDS` | The charge exceeds the free balance or the balance |
| 44 | `BUDGET_BELOW_RESERVED` | A new budget below what is already reserved or spent |
| 45 | `NOTHING_TO_SWEEP` | Sweep with a zero balance |
| 46 | `FEE_EXCEEDS_CAP` | The fee exceeds the fee cap for the total |
| 50 | `RECIPIENT_NOT_ALLOWED` | A leg destination is not on the allowlist for the recipient |
| 51 | `ATTESTATION_INVALID` | An attestation where none is allowed, the wrong number of attestations, a window longer than `MAX_ATTESTATION_VALIDITY`, or a signature that is not 64 bytes. A wrongly signed attestation is a host trap on Stellar, not this code (`MANDATE-RECIPIENT-012`) |
| 52 | `ATTESTATION_EXPIRED` | An attestation at or after its `valid_until` |
| 53 | `POLICY_CHANGED` | The mandate's policy epoch differs from the epoch the attestation was issued under: at request (`MANDATE-RECIPIENT-019`) or at execute (`MANDATE-RECIPIENT-015`) |
| 54 | `WRONG_RECIPIENT_MODE` | A call that the recipient mode does not use |
| 60 | `PAYOUT_ID_EXISTS` | The payout ID was already used |
| 61 | `PAYOUT_NOT_FOUND` | No payout with that ID |
| 62 | `PAYOUT_WRONG_STATUS` | The payout is not in a status this call accepts |
| 63 | `PAYOUT_LAPSED` | The payout is at or after its own expiry |
| 64 | `ALREADY_APPROVED` | A repeated approval or recovery vote |
| 65 | `AGENT_REMOVED` | The agent that requested the payout is no longer an agent |
| 70 | `ASSET_NOT_SUPPORTED` | The factory does not support the asset |
| 71 | `WRONG_ASSET` | Token recovery of the settlement asset |
| 72 | `TRANSFER_FAILED` | A token transfer out of the mandate failed |
| 90 | `INTERNAL_INVARIANT` | An arithmetic failure the rules make unreachable |

Not errors of the mandate: a missing signature on Stellar (a host authorization failure), an out-of-gas on Celo, and resource or budget exhaustion on Stellar. These are transport failures: the call does not land and nothing changes.

## Roles

| Role | Holder | May |
| --- | --- | --- |
| Principal | The paying business: a Stellar account or contract (for example a smart wallet or multisig), a Celo account or contract (for example a Safe) | Every administrative action, plus cancel, reject, execute, pause, withdraw, sweep and token recovery; approve only if also an approver. May also be a guardian; never an agent, executor or recovery approver |
| Agent | Spending keys, up to `MAX_AGENTS` | Request payouts, cancel non-terminal payouts, execute approved payouts |
| Approver | Named people or their wallets, up to `MAX_APPROVERS` | Approve or reject pending payouts, pause |
| Guardian | Optional, up to `MAX_GUARDIANS`, for example an operator's pause key | Pause only |
| Executor | Optional, up to `MAX_EXECUTORS`, for example the operator's key that pays gas | Execute approved payouts |
| Recovery approver | Optional, up to `MAX_RECOVERY`, people the principal trusts to act if its key is lost | Alone: pause. With `recovery_threshold` votes: revoke, or rotate one agent. Nothing else |
| Attester | An ed25519 public key the principal chooses; never a caller | Sign recipient eligibility attestations |
| Anyone | Any address | Send tokens to the mandate, cancel lapsed payouts, sweep a closed mandate to the principal within the sweep guards |

#### MANDATE-ROLE-001 Authority comes from role addresses only

- Precondition: any call that a role restricts.
- Effect: the caller is identified by its own address: on Stellar by `require_auth` on that address, on Celo by `msg.sender`. The transaction source, the fee payer and `tx.origin` are never read, so paying someone's fee never grants authority. On Celo there are no signed meta-transactions in spec version 1.
- Error: none (the role checks below return the errors).

#### MANDATE-ROLE-002 The principal is never an agent or an executor

- Precondition: a configuration or role change that would put the principal in `agents` or `executors`.
- Effect: refused. The principal already executes, and requesting payouts is deliberately left to agent keys, so the principal's key is never used for routine spending and each key has one clear job.
- Error: `ROLE_CONFLICT`

#### MANDATE-ROLE-003 Agents and approvers are disjoint

- Precondition: a configuration or role change that would put one address in both `agents` and `approvers`.
- Effect: refused, so an agent can never approve its own request.
- Error: `ROLE_CONFLICT`

#### MANDATE-ROLE-004 Executors and approvers are disjoint

- Precondition: a configuration or role change that would put one address in both `executors` and `approvers`.
- Effect: refused, so the party that submits cannot also be the approval gate.
- Error: `ROLE_CONFLICT`

#### MANDATE-ROLE-005 Recovery approvers are disjoint from agents and executors

- Precondition: a configuration or role change that would put one address in `recovery` and in `agents` or `executors`.
- Effect: refused, so a spending or submitting key can never rotate agents or revoke.
- Error: `ROLE_CONFLICT`

#### MANDATE-ROLE-006 Allowed overlaps

- Precondition: none.
- Effect: these overlaps, and only these, are allowed: the principal may also be an approver or a guardian; a guardian may also be an approver, an agent, an executor or a recovery approver; a recovery approver may also be an approver or a guardian. Pausing only tightens, so these overlaps grant nothing dangerous. Every other pair of roles is disjoint (`MANDATE-ROLE-002` to `MANDATE-ROLE-005`, `MANDATE-ROLE-020`, `MANDATE-ROLE-021`), and the fee address holds no role at all (`MANDATE-ROLE-008`). Payout approvals and recovery votes never count for each other. Accepted property: a recovery quorum whose members are also approvers can rotate in an agent and then approve that agent's payouts above the threshold without the principal. This is intended, because the recovery set exists to act when the principal cannot, and it does not widen where money can go: destinations stay bound by the principal's allowlist or the attester, and every limit still applies.
- Error: none

#### MANDATE-ROLE-007 Role sets are bounded

- Precondition: a configuration or `add_role` that would make `agents`, `approvers`, `guardians`, `executors` or `recovery` larger than its bound.
- Effect: refused. Every loop over a role set is therefore bounded.
- Error: `ROLE_SET_FULL`

#### MANDATE-ROLE-008 The fee address holds no role

- Precondition: a configuration, `add_role`, principal proposal or acceptance, or agent rotation that would make `fee_address` the principal or put it in `agents`, `approvers`, `guardians`, `executors` or `recovery`.
- Effect: refused. The fee address is used for nothing else, not even pausing; the operator's pause key is a separate guardian.
- Error: `ROLE_CONFLICT`

#### MANDATE-ROLE-009 Role conflict check on add

- Precondition: `add_role(role, account)` where adding `account` would break, checked in this order, `MANDATE-ROLE-002`, `MANDATE-ROLE-003`, `MANDATE-ROLE-004`, `MANDATE-ROLE-005`, `MANDATE-ROLE-020`, `MANDATE-ROLE-021` or `MANDATE-ROLE-008`.
- Effect: refused. The attester overlap is a separate check (`MANDATE-ROLE-018`).
- Error: `ROLE_CONFLICT`

#### MANDATE-ROLE-010 Agent-only calls

- Precondition: `request_payout` or `request_payouts` from a caller not in `agents`.
- Effect: refused.
- Error: `UNAUTHORIZED`

#### MANDATE-ROLE-011 Approver-only calls

- Precondition: `approve` from a caller not in `approvers`.
- Effect: refused.
- Error: `UNAUTHORIZED`

#### MANDATE-ROLE-012 Who may reject

- Precondition: `reject` from a caller that is neither in `approvers` nor the principal.
- Effect: refused.
- Error: `UNAUTHORIZED`

#### MANDATE-ROLE-013 Who may execute

- Precondition: `execute` or `execute_batch` from a caller that is not the principal and not in `agents` or `executors`.
- Effect: refused.
- Error: `UNAUTHORIZED`

#### MANDATE-ROLE-014 Who may pause

- Precondition: `pause` from a caller that is not the principal and not in `approvers`, `guardians` or `recovery`.
- Effect: refused.
- Error: `UNAUTHORIZED`

#### MANDATE-ROLE-015 Principal-only calls

- Precondition: a principal-only call (`resume`, `revoke`, `withdraw`, `recover_token`, `update_limits`, `add_role`, `remove_role`, `set_approvals_required`, `set_recovery_threshold`, `allow_recipients`, `disallow_recipients`, `set_attestation_policy`, `propose_principal`, `cancel_principal_proposal`, `lower_fee_cap`) from any other caller.
- Effect: refused. On Stellar these calls take no caller argument and require the principal's authorization, so another caller fails as a host authorization failure; on Celo `msg.sender` must be the principal.
- Error: `UNAUTHORIZED` (Celo); host authorization failure (Stellar)

#### MANDATE-ROLE-016 Role change validation

- Precondition: `add_role` or `remove_role` with a role code outside `1 AGENT`, `2 APPROVER`, `3 GUARDIAN`, `4 EXECUTOR`, `5 RECOVERY`; adding an address already in the set; or removing one not in it.
- Effect: refused.
- Error: `CONFIG_INVALID`

#### MANDATE-ROLE-017 No zero address as principal or role member

- Precondition: on Celo, the zero address given as `config.principal`, as any role member at creation, in `add_role`, or as `propose_principal(proposed)`. In `recovery_rotate_agent` the zero address encodes "no new agent", so it never reaches this rule.
- Effect: refused. The zero address can never call, so as a role member it would count toward `len(approvers)` or `len(recovery)` without ever being able to approve or vote, and as a proposed principal it could not be told apart from no proposal. Unreachable on Stellar, which has no zero address; the vectors are Celo only with the unrepresentable-input skip reason.
- Error: `CONFIG_INVALID`

#### MANDATE-ROLE-018 The attester's key holds no role

- Precondition: when an attester is configured, the `attester` key equals the ed25519 public key of the principal, of `fee_address` or of a member of `agents`, `approvers`, `guardians`, `executors` or `recovery`, where that address is an account (`G`) address; a contract (`C`) address has no such key and never matches. Checked on every path that can create the overlap: at creation, in `set_attestation_policy`, `add_role`, `propose_principal`, `accept_principal` and `recovery_rotate_agent`.
- Effect: refused, so the attester key is never the master key of a role account, whichever of the two is set first. On Stellar an account's authority comes from its signers, which this check does not read, so the stronger guarantee that the attester key signs for no role account at all is off chain (`CHAINOPS-CREATE-004`).
- Error: `ROLE_CONFLICT`

#### MANDATE-ROLE-019 Every caller argument is authorized first

- Precondition: a call that takes a `caller` address, including one that anyone may make (`cancel`, `cancel_batch`, `sweep`).
- Effect: on Stellar the first check is `caller.require_auth()`, before any lookup or role decision; on Celo the caller is `msg.sender`. Without it, an account could pass the principal's address to get past the sweep guards, cancel as an agent, or forge the `by` field of an event. Vectors on Stellar include a call that names the principal as `caller` without the principal's signature.
- Error: host authorization failure (Stellar); not reachable on Celo

#### MANDATE-ROLE-020 Agents and executors are disjoint

- Precondition: a configuration, `add_role` or agent rotation that would put one address in both `agents` and `executors`.
- Effect: refused. An agent already executes, so the overlap adds no power and only blurs which key did what.
- Error: `ROLE_CONFLICT`

#### MANDATE-ROLE-021 The principal is not a recovery approver

- Precondition: a configuration, `add_role(5, account)`, principal proposal or acceptance that would put the principal in `recovery`.
- Effect: refused. The recovery set exists for when the principal's key is lost, so a principal seat in it would be a vote lost at exactly that moment and would overstate the set's real strength.
- Error: `ROLE_CONFLICT`

### Recovery approver set

#### MANDATE-RECOVERYSET-001 Only the principal changes the set

- Precondition: none.
- Effect: members are added with `add_role(5, account)` and removed with `remove_role(5, account)`, by the principal only. The set can never change itself or its threshold. If the principal's key is lost, the set stays as it was.
- Error: none

#### MANDATE-RECOVERYSET-002 Threshold bounds

- Precondition: a configuration, `set_recovery_threshold(n)` or `remove_role(5, account)` that would leave `recovery_threshold` other than 0 while `recovery` is empty, or outside `[1, len(recovery)]` while it is not.
- Effect: refused.
- Error: `CONFIG_INVALID`

#### MANDATE-RECOVERYSET-003 Recovery epoch

- Precondition: an executed recovery action, a change to `recovery` or `recovery_threshold`, or any change to `agents`.
- Effect: `recovery_epoch` increases by one. Votes are kept per epoch and action, so votes cast before the change can never complete later.
- Error: none

#### MANDATE-RECOVERYSET-004 Recovery-only calls

- Precondition: `recovery_revoke` or `recovery_rotate_agent` from a caller not in `recovery`.
- Effect: refused.
- Error: `UNAUTHORIZED`

#### MANDATE-RECOVERYSET-005 Rotation targets an existing agent

- Precondition: `recovery_rotate_agent(old, new)` where `old` is not a current agent, or `new` is given and is already an agent.
- Effect: refused. One agent leaves as one joins, so the agent bound cannot be exceeded.
- Error: `CONFIG_INVALID`

#### MANDATE-RECOVERYSET-006 Rotation respects the role rules

- Precondition: `recovery_rotate_agent(old, new)` where `new` is in `approvers`, `executors` or `recovery`, or is the principal or `fee_address`.
- Effect: refused; the same separation as `add_role` (`MANDATE-ROLE-009`). The attester key overlap is a separate check (`MANDATE-ROLE-018`). A proposed principal that becomes an agent this way cannot then accept (`MANDATE-PRINCIPAL-006`).
- Error: `ROLE_CONFLICT`

#### MANDATE-RECOVERYSET-007 One vote per member per action and epoch

- Precondition: a member votes again for the same action in the current `recovery_epoch`.
- Effect: refused.
- Error: `ALREADY_APPROVED`

#### MANDATE-RECOVERYSET-008 Voting and execution

- Precondition: all checks of the recovery call pass.
- Effect: record the vote and emit `recovery_vote_added { kind, old, new, votes, required, recovery_epoch }`, where `kind` is `1 REVOKE` or `2 ROTATE_AGENT`. When the votes of distinct current members reach `recovery_threshold`, perform the action exactly as the principal's own path would (the revoke effects of `MANDATE-REVOKE-001`, or `role_removed` for `old` and, if `new` is given, `role_added` for `new`, where the new agent takes the next `agent_epoch` value: the counter increases first and the agent stores the new value, `MANDATE-PAYOUT-014`), increase `recovery_epoch`, and emit `recovery_executed { kind, old, new, by }` last. For a revoke, `old` and `new` are empty. Returns `1` when a vote was recorded and `2` when the action ran.
- Error: none

#### MANDATE-RECOVERYSET-009 Recovery powers are limited

- Precondition: none.
- Effect: a single member may pause. With the threshold the set may revoke (which freezes the balance for the principal and moves no funds) or rotate one agent. Recovery never withdraws, never changes a destination, the allowlist, the attestation policy, a limit, the principal, the approvers, the guardians, the executors or the recovery set itself. Payout approvals never count as recovery votes and recovery votes never approve a payout. Recovery calls are allowed while paused and after expiry, because recovery never widens where money can go or any limit: a revoke only freezes, and a rotation only exchanges one agent for another under the same bound destinations and limits (`MANDATE-ROLE-006` records what a rotation followed by approvals can do).
- Error: none

## State

#### MANDATE-CONFIG-001 Immutable fields

- Precondition: none.
- Effect: fixed at construction and never changed: `asset` (the single settlement asset, which the factory must support), `recipient_mode` (`1 ALLOWLIST`, `2 ATTESTED`, `3 BOTH`), `tenant_ref` (an opaque 32-byte reference that lets the operator attribute events), `factory` (the creating factory) and `fee_address` (copied from the factory, never supplied by the principal).
- Error: none

#### MANDATE-CONFIG-002 Field domains and cross-field rules

- Precondition: a configuration at creation or a later change to any of these fields with a value outside its rule:
  - `total_budget` in `[1, I128_MAX]`;
  - `per_payment_cap` in `[1, total_budget]`;
  - `per_recipient_daily_cap` in `[1, I128_MAX]`;
  - `approval_threshold` in `[0, I128_MAX]` (0 means every payout needs approval);
  - `payout_ttl_seconds` in `[PAYOUT_TTL_MIN, PAYOUT_TTL_MAX]`;
  - `expires_at` greater than `now`;
  - `max_fee_bps` in `[0, MAX_FEE_BPS]`;
  - `country_policy_hash`: 32 bytes, the SHA-256 of the canonical sorted, comma-joined list of ISO 3166-1 alpha-2 codes the principal chose (the contract never sees the list).
- Effect: refused. This rule covers field domains only; a budget below what is already reserved or spent is `MANDATE-BUDGET-002`. `approvals_required` (`MANDATE-APPROVAL-002`), `recovery_threshold` (`MANDATE-RECOVERYSET-002`) and the attester (`MANDATE-CONFIG-005`) are their own checks, so every check order cites each of them at exactly one step.
- Error: `CONFIG_INVALID`

#### MANDATE-CONFIG-003 Caller of create is the principal

- Precondition: `create(config, salt)` from a caller other than `config.principal`.
- Effect: refused. `create` takes no caller argument: on Stellar it requires the authorization of `config.principal`, so another caller fails as a host authorization failure; on Celo `msg.sender` must be `config.principal`.
- Error: `UNAUTHORIZED` (Celo); host authorization failure (Stellar)

#### MANDATE-CONFIG-004 The factory validates before it deploys

- Precondition: `create` passed the caller and asset checks.
- Effect: the factory validates the full configuration before deploying, in the fixed order of `create` checks 3 to 16, and returns the first error itself. The constructor repeats the same checks in the same order as a backstop, so two implementations can never pick a different first error for a configuration that breaks several rules. Both chains follow this structure, so the error code always comes from the factory. On Stellar a constructor error reaches the factory's caller only as a generic host error, so a vector that sees that from `create` has found a factory bug. The Stellar vector runner therefore loads the compiled contract, not a native test registration, because host error codes seen through native test contracts differ from what the chain reports; the codes the mandate itself returns are the same either way. A failed create leaves no contract and no events, and retrying with the same salt is safe.
- Error: `CONFIG_INVALID`, `ROLE_CONFLICT`, `ROLE_SET_FULL`

#### MANDATE-CONFIG-005 Attester presence and form

- Precondition: a configuration where `recipient_mode` is `ATTESTED` or `BOTH` and `attester` is missing, or `ALLOWLIST` and `attester` is set; or an `attester` that is not exactly 32 bytes (an ed25519 public key) or is 32 zero bytes. The attester is passed as a variable-length byte string so this check can run; whether the bytes are a valid curve point is checked off chain (`CHAINOPS-CREATE-005`).
- Effect: refused.
- Error: `CONFIG_INVALID`

#### MANDATE-CONFIG-006 Deterministic address and creation events

- Precondition: `create` passed every check.
- Effect: the mandate is deployed at a deterministic address: on Stellar by the deployer with salt `SHA-256(principal_xdr ‖ salt)`, on Celo by `CREATE2` with salt `keccak256(abi.encode(principal, salt))`. The constructor sets `policy_epoch = 0`, `paused = false`, `revoked = false`, `recovery_epoch = 0` and `agent_epoch = 0`, and stores every agent of the configuration with epoch 0 (`MANDATE-PAYOUT-014`). The factory emits `mandate_created` and the mandate emits `mandate_initialized`.
- Error: none

#### MANDATE-CONFIG-007 update_limits

- Precondition: `update_limits(limits)` with `limits = { total_budget, per_payment_cap, per_recipient_daily_cap, approval_threshold, payout_ttl_seconds, expires_at }` where a field breaks `MANDATE-CONFIG-002`, or `expires_at <= now`.
- Effect: refused. On success all six fields are replaced at once and `limits_updated` is emitted with the new values. A new `payout_ttl_seconds` applies to new requests only, and so does a new `approval_threshold` (`MANDATE-APPROVAL-001`). With no approvers, the threshold rule is re-checked here (`MANDATE-APPROVAL-009`). Extending `expires_at` after expiry makes the mandate active again, subject to `MANDATE-FUND-004`.
- Error: `CONFIG_INVALID`

#### MANDATE-CONFIG-008 set_attestation_policy

- Precondition: `set_attestation_policy(attester, country_policy_hash)` while the mode is `ALLOWLIST`, or with an attester that breaks `MANDATE-CONFIG-005`.
- Effect: refused. The attester key overlap is a separate check (`MANDATE-ROLE-018`). On success `attester` and `country_policy_hash` are replaced, `policy_epoch` increases by one, and `attestation_policy_set { attester, country_policy_hash, policy_epoch }` is emitted.
- Error: `WRONG_RECIPIENT_MODE`, `CONFIG_INVALID`

#### MANDATE-CONFIG-009 Configuration changes are atomic

- Precondition: any configuration call, including the batch ones.
- Effect: a configuration call either applies completely or not at all, because it is one administrative intent.
- Error: none

#### MANDATE-CONFIG-010 Derived values

- Precondition: none.
- Effect: computed on read, never stored:
  - `balance = min(asset.balance(mandate), I128_MAX)`;
  - `active = !revoked && now < expires_at`;
  - `expired = now >= expires_at`;
  - `free = active ? max(0, balance - pending - committed) : balance`.
- Error: none

#### MANDATE-CONFIG-011 Views

- Precondition: none.
- Effect: `version()` returns `SPEC_VERSION`; `config()` returns the configuration; `state()` returns `{ spent, pending, committed, balance, free, paused, revoked, expired, policy_epoch }`; `payout(id)` returns the record, the tombstone or nothing; `daily_used(recipient_ref)` returns `{ day, used }` with `used = 0` when the stored day is not today; `is_allowed(recipient_ref, destination)` answers for the destination pair.
- Error: none

### Payout records

A payout record holds `payout_id`, `agent`, `agent_epoch` (the epoch under which that agent was added, `MANDATE-PAYOUT-014`), `recipient_ref`, `total`, `fee`, `legs` (1 to `MAX_LEGS` entries of `{ destination, amount }`), `status` (`1 PENDING`, `2 APPROVED`, `3 EXECUTED`, `4 REJECTED`, `5 CANCELLED`), `created_at`, `expires_at`, `policy_epoch` and the distinct `approvals`.

#### MANDATE-PAYOUT-001 Tombstones keep every ID used forever

- Precondition: a payout reaches EXECUTED, REJECTED or CANCELLED.
- Effect: the record is replaced by a tombstone `{ payout_id, status }` that is kept forever, so an ID is never reused and a terminal status never changes.
- Error: none

#### MANDATE-PAYOUT-002 A used ID is refused

- Precondition: a request whose `payout_id` already has a record or a tombstone.
- Effect: refused. This check comes early so that a retry of a request that already landed gets the most useful answer.
- Error: `PAYOUT_ID_EXISTS`

#### MANDATE-PAYOUT-003 Request deadline

- Precondition: a request with `now >= deadline`.
- Effect: refused. The deadline makes it decidable when an attempt that never landed is dead.
- Error: `REQUEST_DEADLINE_PASSED`

#### MANDATE-PAYOUT-004 Unknown payout

- Precondition: `approve`, `reject`, `cancel` or `execute` for an ID with no record and no tombstone.
- Effect: refused.
- Error: `PAYOUT_NOT_FOUND`

#### MANDATE-PAYOUT-005 Status the call accepts

- Precondition: `approve` or `reject` on a payout that is not PENDING; `cancel` on one that is neither PENDING nor APPROVED; `execute` on one that is not APPROVED.
- Effect: refused.
- Error: `PAYOUT_WRONG_STATUS`

#### MANDATE-PAYOUT-006 Lapsed payout

- Precondition: `approve` or `execute` with `now >= payout.expires_at`.
- Effect: refused. A payout is live while `now < payout.expires_at`.
- Error: `PAYOUT_LAPSED`

#### MANDATE-PAYOUT-007 Request effects

- Precondition: a request passed every check.
- Effect: store the record with `created_at = now`, `expires_at` as in `MANDATE-EXPIRY-003` and `MANDATE-EXPIRY-004`, the current `policy_epoch`, and the requesting agent's stored epoch as `agent_epoch` (`MANDATE-PAYOUT-014`); on Stellar, set the record's TTL under `MANDATE-PAYOUT-016`. The status follows `MANDATE-APPROVAL-001`. Emit `payout_requested`. Return the status. Funds are reserved at request so that `committed` always means covered by the balance and the principal can see and veto what is queued.
- Error: none

#### MANDATE-PAYOUT-008 IDs are opaque to the contract

- Precondition: none.
- Effect: `payout_id`, `recipient_ref`, `task_ref` and `tenant_ref` are 32-byte values derived off chain with domain tags. The contract only stores and compares them. `task_ref` is carried in the request and in `payout_requested` and is never stored.
- Error: none

#### MANDATE-PAYOUT-009 Archived entries are never absent

- Precondition: on Stellar, a payout record, tombstone or other persistent entry that has been archived, including after eviction.
- Effect: it counts as present. A transaction built from simulation restores it, so `PAYOUT_ID_EXISTS` and every other rule behaves as for a live entry; a transaction built without the restore fails as a transport failure and changes nothing. Rule state never uses temporary storage, because an expired temporary entry reads as absent.
- Error: none

#### MANDATE-PAYOUT-010 Who may cancel

- Precondition: `cancel` where the caller is not the principal (reason 1), is not the payout's own agent while it is still the same agent under `MANDATE-PAYOUT-014` (reason 2), and neither of these holds for anyone else (reason 3): `now >= payout.expires_at`; or the mandate is expired and `now - expires_at >= SWEEP_GRACE_SECONDS`. A payout whose own time to live lapsed can be cancelled by anyone at once; a cancel that rests only on the mandate's expiry waits out the grace. When the mandate is expired `now >= expires_at`, so the subtraction cannot underflow.
- Effect: refused. Authorization is checked after the lookup here because who may cancel depends on the payout. Reason 2 is limited to the payout's own agent so that one agent, for example an operator-held key, can never cancel payouts that people approved for another.
- Error: `UNAUTHORIZED`

#### MANDATE-PAYOUT-011 Cancel effects

- Precondition: `cancel` passed every check.
- Effect: tombstone CANCELLED; release `pending` or `committed` by `total + fee`; no fee is paid; emit `payout_cancelled { payout_id, by, reason }` with reason `1` principal, `2` agent or `3` lapsed.
- Error: none

#### MANDATE-PAYOUT-012 Execute effects

- Precondition: an execute item passed every check.
- Effect: in this order, tombstone EXECUTED; `committed -= total + fee`; `spent += total + fee`; `daily[recipient_ref] = { today, used + total }`; emit `payout_executed`; then transfer each leg's amount to its destination in leg order, and last, if `fee > 0`, transfer `fee` to `fee_address`. All of it is one atomic step: if any transfer fails the whole call reverts, every state change and event above included.
- Error: `TRANSFER_FAILED`

#### MANDATE-PAYOUT-013 Transfers out are wrapped

- Precondition: any transfer out of the mandate: legs, the fee, `withdraw`, `sweep` and `recover_token` (`revoke` makes none).
- Effect: on Stellar each transfer is made through a guarded contract call, and any error it returns, of any type, becomes `TRANSFER_FAILED`, which reverts every effect of the call, including legs already paid; a token `transfer` that returns a value instead of unit is also `TRANSFER_FAILED`. Budget or storage-limit exhaustion and host internal errors inside the token cannot be caught and fail the call as a transport failure. On Celo a revert, a `false` return or missing return data from the settlement asset is `TRANSFER_FAILED`; for `recover_token` only, a transfer succeeds when the call does not revert, the token has code, and the return data is empty or decodes to `true`, so tokens that return nothing can still be recovered. The mandate never branches on the token's own error codes. Balance reads of the settlement asset are not transfers: on Stellar a failed read is a host abort (the asset contract cannot fail it other than by budget exhaustion, or by an archived balance entry the transaction did not restore, which fails as `EntryArchived`), and on Celo a reverting `balanceOf` reverts the whole call without a mandate code; on both chains it is a transport failure, reported to the operator as not landed, never as `TRANSFER_FAILED`.
- Error: `TRANSFER_FAILED`

#### MANDATE-PAYOUT-014 The requesting agent is still the same agent

- Precondition: `approve` or `execute` of a payout whose `payout.agent` is no longer in `agents`, or was removed and added again since the request.
- Effect: refused, so removing or rotating out a compromised agent stops every payout it queued, including when the principal's key is lost and only the recovery set can act, and adding the same address back later does not revive them. The mandate keeps a counter `agent_epoch` and stores, for each agent, the value it was added under; each payout record copies its agent's value at request (`MANDATE-PAYOUT-007`). Epoch values: `agent_epoch` starts at 0 and every agent named in the creation configuration is stored with 0; every later addition, by `add_role(1, ...)` or as the `new` agent of a rotation, first increases the counter by one and then stores the new value. So an agent added after creation, or added back after a removal, always holds a value of 1 or more that no payout queued before that addition carries. The check is that `payout.agent` is an agent and its stored value equals `payout.agent_epoch`. Such payouts can still be cancelled by the principal, or by anyone once they lapse. Cancel reason 2 uses the same test.
- Error: `AGENT_REMOVED`

#### MANDATE-PAYOUT-015 No nested call into the mandate during a transfer

- Precondition: while the mandate is making a token transfer (a leg, the fee, `withdraw`, `sweep` or `recover_token`), the token or anything it calls tries to call the same mandate: any state-changing function on Celo, any function at all on Stellar.
- Effect: the nested call fails and changes nothing. On Celo every state-changing function carries one reentrancy guard, a transient-storage flag, and views stay callable, so a view read during a callback may show mid-call state and off-chain code must never treat it as final; on Stellar the host refuses any call into a contract already on the call stack, views included. A token that lets that failure propagate fails its own call, so the outer call fails with `TRANSFER_FAILED` (`MANDATE-PAYOUT-013`). A token that catches it carries on without the nested effect, and the outer call completes as if no nested call had been tried. Either way no reasoning about a call's effects has to consider a nested call from a token, which matters most for `recover_token`, the one call to a token the principal chooses. Balance reads are not covered here: on Celo `balanceOf` is a static call that cannot change state, and on Stellar the settlement asset contract calls nothing back.
- Error: `TRANSFER_FAILED` (the outer call, when the token propagates the failure)

#### MANDATE-PAYOUT-016 Payout record lifetime on Stellar

- Precondition: on Stellar, a request stores a payout record.
- Effect: the request extends the record's TTL so that it stays live until at least `payout.expires_at + RECORD_TTL_MARGIN_SECONDS`, that is, by `ceil((payout.expires_at + RECORD_TTL_MARGIN_SECONDS - now) / STELLAR_LEDGER_SECONDS)` ledgers, clamped to the network's maximum entry TTL. A new persistent entry already starts at the network's minimum TTL and an extension never shortens one, so the TTL is never below that minimum. At the network's current close time the record does not fall short; if the network later closes ledgers faster, a record may archive somewhat before that point, which only costs a restore fee on its next use (`MANDATE-PAYOUT-009`). The submitter pays the rent. Nothing extends a record after that: the tombstone a terminal transition writes keeps the record's remaining TTL and is then left to archive, because an archived record or tombstone still counts as present (`MANDATE-PAYOUT-009`) and a later restore costs only a fee. Leaving tombstones to archive is safe only while an archived tombstone still blocks its key. The signal to revisit this rule is any protocol change that moves archived state out of validator storage so that restoring needs an outside proof, or that lets a new entry be created at an archived key without restoring it first, because either would let a payout ID be reused. The one-day margin covers ledger clock drift, a last-moment execute, and the operator's read of the record after the payout lapses, without a restore. Celo storage does not expire, so the rule has no effect there.
- Error: none

## Amounts, time and identity

#### MANDATE-AMOUNT-001 Amounts are integer smallest units

- Precondition: none.
- Effect: every amount is an integer count of the asset's smallest unit. The contract never knows or reads decimals. On Celo amounts are stored as `uint128` and function arguments are `uint256`, so a value above `I128_MAX` still reaches the domain check.
- Error: none

#### MANDATE-AMOUNT-002 Amount domain

- Precondition: an amount argument outside `[1, I128_MAX]`, except `approval_threshold`, whose domain is `[0, I128_MAX]`, and `fee`, whose domain is `[0, I128_MAX]`.
- Effect: refused. Inside a configuration value the error is `CONFIG_INVALID` (`MANDATE-CONFIG-002`).
- Error: `AMOUNT_INVALID`

#### MANDATE-AMOUNT-003 Only addition, subtraction and comparison

- Precondition: none.
- Effect: the contract only adds, subtracts and compares amounts, with one exception, the fee cap (`MANDATE-FEE-002`). There is no other multiplication, division, percentage or rounding.
- Error: none

#### MANDATE-AMOUNT-004 Request amounts

- Precondition: a request with `total` outside `[1, I128_MAX]`, `fee` outside `[0, I128_MAX]`, or `fee > I128_MAX - total`.
- Effect: refused, so `charged = total + fee` always fits in `i128`.
- Error: `AMOUNT_INVALID`

#### MANDATE-AMOUNT-005 Exact formulas

- Precondition: none.
- Effect: both chains use these exact forms, so they fail on the same edge (a naive `spent + amount > budget` overflows `i128` on Stellar but not `uint256` on Celo):
  - budget: `reserved = spent + pending + committed`; fail if `charged > total_budget - reserved`;
  - daily: `used = (daily.day == today) ? daily.used : 0`; `headroom = max(0, cap - used)`; fail if `total > headroom`;
  - free funds: fail if `charged > free`;
  - legs: `running = 0`; for each leg fail if `leg.amount > total - running`, else `running += leg.amount`; after the loop fail if `running != total`.
- Error: none (each use cites its own error)

#### MANDATE-AMOUNT-006 Unreachable arithmetic

- Precondition: a checked arithmetic operation fails where the formulas above make that impossible.
- Effect: the call fails with a code, never a panic.
- Error: `INTERNAL_INVARIANT`

#### MANDATE-EXPIRY-001 Time

- Precondition: none.
- Effect: `now` takes one value per transaction, so every check in a call, and every item of a batch, sees the same `now`. `today = now / 86400` (integer division) is the UTC calendar day.
- Error: none

#### MANDATE-EXPIRY-002 Expired mandates refuse new work

- Precondition: a call guarded by the active checks while `now >= expires_at`.
- Effect: refused. A mandate is active while `now < expires_at`; at `now == expires_at` it is expired. After expiry open payouts cannot execute; they can be cancelled under `MANDATE-PAYOUT-010`, and the balance returns under the sweep rules.
- Error: `MANDATE_EXPIRED`

#### MANDATE-EXPIRY-003 Payout time to live

- Precondition: none.
- Effect: each payout expires at `created_at + payout_ttl_seconds`, using the value in force at request time, unless `MANDATE-EXPIRY-004` sets it earlier.
- Error: none

#### MANDATE-EXPIRY-004 Attested payouts expire with their attestations

- Precondition: a request in mode `ATTESTED` or `BOTH`.
- Effect: `payout.expires_at = min(now + payout_ttl_seconds, the earliest valid_until among the leg attestations)`. An attestation is valid for at most `MAX_ATTESTATION_VALIDITY`, so an attested payout must be approved and executed while its eligibility statement holds; afterwards it lapses (`MANDATE-PAYOUT-006`), anyone may cancel it, and the agent requests again with fresh attestations. Chosen over storing the earliest `valid_until` and re-checking it at execute, because capping the expiry needs no new field and no new check, and a lapsed payout can be cancelled at once, which releases its reservation instead of leaving it stuck as APPROVED.
- Error: none

## Destinations, legs and recipient binding

#### MANDATE-RECIPIENT-001 A destination is an address and an optional tag

- Precondition: none.
- Effect: a destination is the pair `(address, tag)`, where `tag` is absent or a `u64`. Two destinations are equal only if the address and the tag, including its presence, are equal. The duplicate-leg check, the allowlist, the execute re-check, the attestation and every event use the pair. Without the tag in the binding, an agent allowed to pay one customer of a pooled exchange or anchor account could choose another customer's sub-account.
- Error: none

#### MANDATE-RECIPIENT-002 Destination form on each chain

- Precondition: none.
- Effect: on Stellar a destination is a muxed-capable address (an `M` address carries the tag as its muxed id) passed straight to the token transfer, which records the tag. On Celo it is `address` plus `hasTag` and `uint64 tag`; the tag is bound and emitted but has no effect on the token transfer. The canonical form requires `tag = 0` when `hasTag` is false.
- Error: none

#### MANDATE-RECIPIENT-003 Allowlist mode takes no attestations

- Precondition: a request in mode `ALLOWLIST` with any attestation.
- Effect: refused.
- Error: `ATTESTATION_INVALID`

#### MANDATE-RECIPIENT-004 One attestation per leg

- Precondition: a request in mode `ATTESTED` or `BOTH` whose attestation count differs from its leg count. This is checked once, before the per-leg checks.
- Effect: refused.
- Error: `ATTESTATION_INVALID`

#### MANDATE-RECIPIENT-005 Leg destination is on the allowlist

- Precondition: in mode `ALLOWLIST` or `BOTH`, a leg whose `(recipient_ref, address, tag)` is not on the allowlist. A different tag at the same address is not on the allowlist.
- Effect: refused.
- Error: `RECIPIENT_NOT_ALLOWED`

#### MANDATE-RECIPIENT-006 Attestation not expired

- Precondition: in mode `ATTESTED` or `BOTH`, a leg's attestation with `now >= valid_until`.
- Effect: refused.
- Error: `ATTESTATION_EXPIRED`

#### MANDATE-RECIPIENT-007 Attestation window

- Precondition: in mode `ATTESTED` or `BOTH`, a leg's attestation with `valid_until - now > MAX_ATTESTATION_VALIDITY`.
- Effect: refused. Evaluated only after the expiry check (`MANDATE-RECIPIENT-006`), so `now < valid_until` and the subtraction cannot underflow, and no `now + MAX_ATTESTATION_VALIDITY` sum is ever formed.
- Error: `ATTESTATION_INVALID`

#### MANDATE-RECIPIENT-008 Signature length

- Precondition: a leg's attestation signature that is not exactly 64 bytes (an ed25519 signature). The signature is a variable-length byte string so this check can run before verification.
- Effect: refused.
- Error: `ATTESTATION_INVALID`

#### MANDATE-RECIPIENT-009 Retired: recovery byte

- Precondition: none.
- Effect: retired in spec version 1 when attestations moved from secp256k1 to ed25519, which has no recovery byte; the ID is never reused.
- Error: none

#### MANDATE-RECIPIENT-010 Retired: r in range

- Precondition: none.
- Effect: retired in spec version 1 with the move to ed25519; ed25519 verification checks the signature encoding itself; the ID is never reused.
- Error: none

#### MANDATE-RECIPIENT-011 Retired: low s

- Precondition: none.
- Effect: retired in spec version 1 with the move to ed25519, whose verification on Stellar is strict and rejects a non-canonical signature; the ID is never reused.
- Error: none

#### MANDATE-RECIPIENT-012 Signed by the attester

- Precondition: a leg's attestation whose signature is not a valid ed25519 signature by the configured `attester` over the attestation message. The message is the 15 ASCII bytes `UJIRA/ATTEST/V2` followed by the SHA-256 of the attested fields, 47 bytes in all, so it can never equal a 32-byte transaction hash. The fields bind the chain, the mandate, `recipient_ref`, `country_policy_hash`, `policy_epoch`, `valid_until` and the destination pair; their byte format, in `spec/ENCODING.md`, gives every field a fixed length or a length prefix, so no two field sequences encode to the same bytes. They do not include the amount or the fee. Version 1, never deployed, was a secp256k1 signature over a keccak digest with the prefix `UJIRA/ATTEST/V1`; the different prefix keeps the two versions apart. A future scheme, for example an EVM signature scheme on another chain, takes its own prefix such as `UJIRA/ATTEST/V3` and never reuses `UJIRA/ATTEST/V2`.
- Effect: the whole call fails with no state change. On Stellar the host's ed25519 verification traps on any failure and the contract cannot catch it, so the failure is a host trap rather than a code, and it rolls back a whole `request_payouts` batch. This is accepted: honest attestations can still fail, after an attester change or through an encoding mismatch between the request builder and the attester, but a policy change is caught per item first (`MANDATE-RECIPIENT-019`), and the operator simulates every attested request, isolates a trapping item and re-attests after a policy change before anything is submitted (`CHAINOPS-SUBMIT-003`). Reusing a valid attestation within its window grants nothing beyond what the agent may already do.
- Error: host trap (Stellar)

#### MANDATE-RECIPIENT-013 Retired: unrecoverable signatures

- Precondition: none.
- Effect: retired in spec version 1 with the move to ed25519; every invalid signature now aborts the call under the signature rule that replaced it; the ID is never reused.
- Error: none

#### MANDATE-RECIPIENT-014 Allowlist still holds at execute

- Precondition: `execute` in mode `ALLOWLIST` or `BOTH` where a leg's `(recipient_ref, address, tag)` is no longer on the allowlist.
- Effect: refused, so removing a destination stops payouts already queued.
- Error: `RECIPIENT_NOT_ALLOWED`

#### MANDATE-RECIPIENT-015 Attestation policy unchanged at execute

- Precondition: `execute` in mode `ATTESTED` or `BOTH` where `payout.policy_epoch != policy_epoch`.
- Effect: refused, so a new attester or country policy stops payouts attested under the old one.
- Error: `POLICY_CHANGED`

#### MANDATE-RECIPIENT-016 Allowlist entry validation

- Precondition: `allow_recipients(entries)` or `disallow_recipients(entries)`, where each entry is `{ recipient_ref, destination }`, and: the mode is `ATTESTED`; or an entry has the zero address, a non-canonical tag or the address of `fee_address` (any tag); or, for allow, an entry is already present, or for disallow, an entry is absent, counting duplicate pairs within the same call.
- Effect: refused. On success the call is atomic and emits one `recipient_allowed` or `recipient_disallowed { recipient_ref, destination }` per entry. The zero address and non-canonical tag checks can only be reached on Celo, as for `MANDATE-SPLIT-003` and `MANDATE-SPLIT-004`; their vectors are Celo only with the unrepresentable-input skip reason.
- Error: `WRONG_RECIPIENT_MODE`, `CONFIG_INVALID`

#### MANDATE-RECIPIENT-017 Policy epoch

- Precondition: none.
- Effect: `policy_epoch` increases on every attester or country policy change. Allowlist removals do not change it, because execute re-checks the allowlist directly.
- Error: none

#### MANDATE-RECIPIENT-018 Choosing the recipient mode

- Precondition: none.
- Effect: mandates that pay per-order off-ramp deposit addresses use `ATTESTED` or `BOTH`, because only the attester can confirm at signing time that such an address belongs to that recipient's order. `ALLOWLIST` alone suits stable wallets the principal lists itself. Mandates where the operator holds an agent key use `ALLOWLIST` or `BOTH` (`CHAINOPS-CREATE-003`).
- Error: none

#### MANDATE-RECIPIENT-019 Attestation names the current policy epoch

- Precondition: in mode `ATTESTED` or `BOTH`, a leg's attestation whose `policy_epoch` differs from the mandate's current `policy_epoch`.
- Effect: refused before the signature is verified, so an attestation issued under an earlier attester or country policy gives a per-item code instead of a trap that would abort an honest batch.
- Error: `POLICY_CHANGED`

#### MANDATE-SPLIT-001 Leg count

- Precondition: a request with fewer than 1 or more than `MAX_LEGS` legs.
- Effect: refused.
- Error: `LEGS_INVALID`

#### MANDATE-SPLIT-002 Leg amount

- Precondition: a request with a leg amount outside `[1, I128_MAX]`.
- Effect: refused.
- Error: `LEGS_INVALID`

#### MANDATE-SPLIT-003 No zero-address leg

- Precondition: a request with a leg whose address is the zero address.
- Effect: refused. Stated for both chains so the rule is identical; it can only be reached on Celo.
- Error: `LEGS_INVALID`

#### MANDATE-SPLIT-004 Canonical tag on legs

- Precondition: a request with a leg where the tag is marked absent but its value is not 0.
- Effect: refused. It can only be reached on Celo; Stellar cannot express the input.
- Error: `LEGS_INVALID`

#### MANDATE-SPLIT-005 No duplicate destination

- Precondition: a request with two legs that have the same `(address, tag)` pair.
- Effect: refused. Two legs to the same address with different tags are allowed when both pairs are bound.
- Error: `LEGS_INVALID`

#### MANDATE-SPLIT-006 Legs sum to the total

- Precondition: a request whose legs fail the running-sum algorithm of `MANDATE-AMOUNT-005`.
- Effect: refused.
- Error: `LEGS_SUM_MISMATCH`

#### MANDATE-SPLIT-007 Limits apply to the payout, not its legs

- Precondition: none.
- Effect: every limit is checked on the payout's `total` or `charged`, never per leg, so splitting a payout never changes which limit applies. Each leg goes straight from the mandate to a destination bound to the recipient.
- Error: none

#### MANDATE-SPLIT-008 No leg to the fee address

- Precondition: a request with a leg whose address is `fee_address`, with any tag.
- Effect: refused, so every unit that reaches the fee address is the fee and stays under the fee cap.
- Error: `LEGS_INVALID`

## Funding and assets

#### MANDATE-FUND-001 Funding is a plain transfer

- Precondition: none.
- Effect: there is no deposit function. Anyone funds a mandate by sending the settlement asset to its address with a plain token transfer; the mandate is not called and emits nothing, and the token's transfer event is the funding record. Every later call reads the balance from the token. On Stellar a contract address is reachable only through the asset contract's `transfer`, not a classic payment. On Celo the mandate has no `receive` or payable fallback, so a plain native send reverts.
- Error: none

#### MANDATE-FUND-002 Free funds at request

- Precondition: a request with `charged > free`.
- Effect: refused.
- Error: `INSUFFICIENT_FUNDS`

#### MANDATE-FUND-003 Balance at execute

- Precondition: an execute item with `charged > balance`.
- Effect: refused.
- Error: `INSUFFICIENT_FUNDS`

#### MANDATE-FUND-004 Reactivation needs the reservations covered

- Precondition: `update_limits` on an expired, unrevoked mandate (`now >= expires_at`) with a new `expires_at > now`, while `balance < pending + committed`.
- Effect: refused. After expiry `free` is the whole balance, so the principal may withdraw it, or after the grace anyone may sweep it, while `pending` and `committed` stay as they were. Reactivating then would leave an active mandate whose reservations are not covered, breaking I2, with approved payouts that could only fail at execute. The principal funds the mandate, or cancels payouts until the reservations fit, before extending. Chosen over cancelling the stale payouts automatically, because the contract keeps no enumerable index of open payouts and so cannot release them in bounded cost, and because silently dropping approved payouts would surprise the principal; over freezing `expires_at` after expiry, because extending a lapsed mandate is a useful path; and over releasing reservations at expiry, because expiry happens without a transaction.
- Error: `INSUFFICIENT_FUNDS`

#### MANDATE-DUST-001 Dust is part of the balance

- Precondition: none.
- Effect: any amount down to one smallest unit is part of the balance. There is no minimum and no counter.
- Error: none

#### MANDATE-ASSET-001 Supported asset

- Precondition: `create` with `config.asset` not in the factory's immutable supported list.
- Effect: refused.
- Error: `ASSET_NOT_SUPPORTED`

#### MANDATE-ASSET-002 Other tokens are never counted

- Precondition: none.
- Effect: any token other than `asset`, including the native asset through its token interface, is never read by the accounting and can never be paid out by an agent. The principal recovers it with `recover_token`.
- Error: none

## Limits

#### MANDATE-BUDGET-001 Budget at request

- Precondition: a request with `charged > total_budget - (spent + pending + committed)`.
- Effect: refused.
- Error: `BUDGET_EXCEEDED`

#### MANDATE-BUDGET-002 Budget never below what is reserved

- Precondition: `update_limits` with `total_budget < spent + pending + committed`.
- Effect: refused. Execute does not re-check the budget, because the charge is already reserved.
- Error: `BUDGET_BELOW_RESERVED`

#### MANDATE-PAYMENT-001 Per-payment cap at request

- Precondition: a request with `charged > per_payment_cap`.
- Effect: refused.
- Error: `PAYMENT_CAP_EXCEEDED`

#### MANDATE-PAYMENT-002 Per-payment cap at execute

- Precondition: an execute item with `charged >` the current `per_payment_cap`.
- Effect: refused, so a principal who lowers the cap also stops payouts already queued.
- Error: `PAYMENT_CAP_EXCEEDED`

#### MANDATE-DAILY-001 Daily usage per recipient

- Precondition: none.
- Effect: the mandate keeps one entry `daily[recipient_ref] = { day, used }` per recipient, the executed totals on UTC day `day`, overwritten when the day changes. It counts `total`, never the fee. The cap is keyed by `recipient_ref`, not by destination: a destination bound under two refs would get two caps. The operator never binds one destination to two recipients of the same principal, which keeps one person to one cap.
- Error: none

#### MANDATE-DAILY-002 Daily cap at request

- Precondition: a request whose `total` exceeds today's headroom (`MANDATE-AMOUNT-005`).
- Effect: refused. This is an early rejection; the binding check is at execute.
- Error: `DAILY_CAP_EXCEEDED`

#### MANDATE-DAILY-003 Daily cap at execute

- Precondition: an execute item whose `total` exceeds today's headroom against executed usage.
- Effect: refused. Two payouts to one recipient in the same `execute_batch` accumulate against the cap.
- Error: `DAILY_CAP_EXCEEDED`

#### MANDATE-APPROVAL-001 Which payouts need approval

- Precondition: a request passed every check.
- Effect: if `charged >= approval_threshold`, the payout is PENDING and `pending += charged`; otherwise it is APPROVED and `committed += charged`. The threshold is read at request only: lowering it later does not pull back payouts already approved below the old value. In an incident the principal pauses, revokes, lowers the per-payment cap or the fee cap (both re-checked at execute), removes a destination, or removes the agent (`MANDATE-PAYOUT-014`).
- Error: none

#### MANDATE-APPROVAL-002 Approvals required

- Precondition: a configuration or `set_approvals_required(n)` that breaks this rule: if `approvers` is empty, `n` must be 0 and `approval_threshold > per_payment_cap` (so no payout can need an approval nobody can give); otherwise `n` must be in `[1, len(approvers)]`.
- Effect: refused. On success `approvals_required_set { required }` is emitted.
- Error: `CONFIG_INVALID`

#### MANDATE-APPROVAL-003 Repeated approval

- Precondition: `approve` from an approver already in `payout.approvals` while the live count is still below `approvals_required`.
- Effect: refused. When the live count already meets the requirement (for example after the principal lowered it), the call proceeds to the transition without adding, so a payout never gets stuck.
- Error: `ALREADY_APPROVED`

#### MANDATE-APPROVAL-004 M of N approval

- Precondition: `approve` passed every check.
- Effect: add the caller to `payout.approvals` and emit `payout_approval_added { payout_id, approver, approvals, required }`. The live count is the number of addresses in `payout.approvals` that are current approvers. When it is at least `approvals_required`, the payout becomes APPROVED, `pending -= charged`, `committed += charged`, and `payout_approved { payout_id }` is emitted.
- Error: none

#### MANDATE-APPROVAL-005 A single rejection is a veto

- Precondition: `reject` passed every check.
- Effect: tombstone REJECTED; `pending -= charged`; no fee is paid; emit `payout_rejected { payout_id, by }`. Allowed while paused and after expiry.
- Error: none

#### MANDATE-APPROVAL-006 Approver removal keeps the gate working

- Precondition: `remove_role(2, account)` of one approver among several that would leave `len(approvers) < approvals_required`.
- Effect: refused; the principal lowers `approvals_required` first. Removing the last approver is `MANDATE-APPROVAL-008`.
- Error: `CONFIG_INVALID`

#### MANDATE-APPROVAL-007 Adding the first approver

- Precondition: `add_role(2, account)` passed every check while `approvers` was empty.
- Effect: `approvals_required` becomes 1 in the same call and `approvals_required_set { required }` is emitted after `role_added`, so `MANDATE-APPROVAL-002` holds after every call. With no approvers `approval_threshold > per_payment_cap`, so no payout needs approval at that moment; the principal lowers the threshold afterwards if it wants approvals.
- Error: none

#### MANDATE-APPROVAL-008 Removing the last approver

- Precondition: `remove_role(2, account)` of the only approver while `approval_threshold <= per_payment_cap`.
- Effect: refused, because payouts could then need an approval nobody can give; the principal raises the threshold above the per-payment cap first. On success `approvals_required` becomes 0 in the same call and `approvals_required_set { required }` is emitted after `role_removed`. PENDING payouts left over can no longer be approved; the principal cancels them, or they lapse.
- Error: `CONFIG_INVALID`

#### MANDATE-APPROVAL-009 Threshold re-checked when there are no approvers

- Precondition: `update_limits` while `approvers` is empty, with a new `approval_threshold <= ` the new `per_payment_cap`.
- Effect: refused, whichever of the two fields changed, so no payout can need an approval nobody can give.
- Error: `CONFIG_INVALID`

## Batches

#### MANDATE-BATCH-001 Request batch length

- Precondition: `request_payouts(reqs)` with `len(reqs)` outside `[1, MAX_BATCH]`.
- Effect: refused; the whole call reverts.
- Error: `BATCH_INVALID`

#### MANDATE-BATCH-002 Request batch runs per item

- Precondition: `request_payouts` passed the caller, active and length checks.
- Effect: each item in order runs the `request_payout` checks from the payout ID check onward against the state left by earlier items. A failing item writes nothing, emits `payout_request_rejected { payout_id, agent, code }`, and the batch continues; a duplicate ID later in the same batch fails with `PAYOUT_ID_EXISTS`. The call returns one `ItemResult { payout_id, code, status }` per item, with code 0 for success. The one exception is `MANDATE-RECIPIENT-012`: an invalid attestation signature aborts the whole call.
- Error: per item (items report their own codes)

#### MANDATE-BATCH-003 Cancel batch

- Precondition: `cancel_batch(ids)` with `len(ids)` outside `[1, MAX_BATCH]`.
- Effect: refused; the whole call reverts. Otherwise each item runs the `cancel` checks from the lookup onward, per item, and an item error never reverts the call.
- Error: `BATCH_INVALID`

#### MANDATE-BATCH-004 Execute batch length

- Precondition: `execute_batch(ids)` with `len(ids)` outside `[1, MAX_BATCH]`.
- Effect: refused.
- Error: `BATCH_INVALID`

#### MANDATE-BATCH-005 Execute batch is atomic

- Precondition: `execute_batch` passed the caller, active and length checks.
- Effect: each ID in order runs the `execute` checks from the lookup onward against the state left by earlier items, then the transfer budget (`MANDATE-BATCH-006`), then its effects, transfers included, before the next item's checks start. So the balance check of a later item reads the balance after earlier items were paid. The first failure reverts the whole call with that item's code. Batches are atomic so that no state change is ever left behind a caught transfer failure.
- Error: per item (items report their own codes)

#### MANDATE-BATCH-006 Transfer budget per execute batch

- Precondition: in `execute_batch`, after an item's own checks, the running count of transfers (each item's legs, plus 1 when its `fee > 0`) exceeds `MAX_BATCH_TRANSFERS`.
- Effect: the batch fails. Because the check runs after the item's own checks, an item that breaks a rule reports that rule first, so precedence is deterministic; because the batch is atomic, a running count is equivalent to a pre-pass. A single `execute` needs no transfer budget (at most `MAX_LEGS + 1 = 5` transfers), and request and cancel batches keep only the item bound.
- Error: `BATCH_INVALID`

#### MANDATE-BATCH-007 Allowlist batch length

- Precondition: `allow_recipients` or `disallow_recipients` with fewer than 1 or more than `MAX_BATCH` entries.
- Effect: refused.
- Error: `BATCH_INVALID`

## Pause, revoke and the return of funds

#### MANDATE-PAUSE-001 Paused mandates refuse new work

- Precondition: a call guarded by the active checks while `paused`.
- Effect: refused. Pause blocks request, approve and execute.
- Error: `MANDATE_PAUSED`

#### MANDATE-PAUSE-002 Pause

- Precondition: `pause` while already paused.
- Effect: refused. On success `paused = true` and `paused { by }` is emitted. Pause never moves money, so no token can block it.
- Error: `ALREADY_PAUSED`

#### MANDATE-PAUSE-003 Resume

- Precondition: `resume` while not paused.
- Effect: refused. On success `paused = false` and `resumed { by }` is emitted.
- Error: `NOT_PAUSED`

#### MANDATE-REVOKE-001 Revoke is a pure state change

- Precondition: `revoke` by the principal, or a recovery revoke at threshold, passed every check. Allowed while paused or expired.
- Effect: `revoked = true`, `pending = 0`, `committed = 0`. Every open payout becomes unexecutable and every role-gated power ends at once, because every agent, approver, executor, guardian and recovery path checks `revoked` first; role storage is kept, so history stays readable and the call costs the same whatever the role set sizes. No token transfer is made; the only token call is a read of the mandate's balance, so no issuer, trustline, token bug or resource estimate can stop it. Emit `revoked { by, frozen }`, where `frozen` is the balance at that moment, informational only, and return it.
- Error: none

#### MANDATE-REVOKE-002 Revoked mandates refuse state changes

- Precondition: a call on a revoked mandate, other than `withdraw`, `sweep`, `recover_token`, `propose_principal`, `accept_principal`, `cancel_principal_proposal` and the views. This includes a second `revoke`.
- Effect: refused. `revoked` never clears.
- Error: `MANDATE_REVOKED`

#### MANDATE-REVOKE-003 Return path after revoke

- Precondition: the mandate is revoked.
- Effect: the balance leaves only through `withdraw` (the principal) and `sweep` (anyone, to the principal). The destination of both is fixed to the principal. Either may fail with `TRANSFER_FAILED` and leave the state unchanged, and either may be retried until it succeeds. If the principal cannot receive (no trustline, deauthorized or blocked by the issuer), it fixes that or moves to a new address with the principal transfer, then retries.
- Error: none

#### MANDATE-WITHDRAW-001 Withdraw goes to the principal only

- Precondition: none.
- Effect: `withdraw(amount)` takes no destination argument and always pays the principal, so a phished signature cannot redirect funds. Allowed while paused, expired and revoked.
- Error: none

#### MANDATE-WITHDRAW-002 Withdraw amount

- Precondition: `withdraw(amount)` with `amount` outside `[1, I128_MAX]`.
- Effect: refused.
- Error: `AMOUNT_INVALID`

#### MANDATE-WITHDRAW-003 Withdraw is capped at the free balance

- Precondition: `withdraw(amount)` with `amount > free`. While active, `free` excludes `pending` and `committed`; once revoked or expired, `free` is the whole balance.
- Effect: refused. To take back reserved funds the principal cancels payouts, or revokes and then withdraws, so an approved payout never fails at execute because funds quietly left.
- Error: `INSUFFICIENT_FUNDS`

#### MANDATE-WITHDRAW-004 Withdraw effects

- Precondition: `withdraw` passed every check.
- Effect: transfer `amount` to the principal and emit `withdrawn { to, amount }`. A failed transfer changes nothing and may be retried.
- Error: `TRANSFER_FAILED`

#### MANDATE-SWEEP-001 Sweep needs a closed mandate

- Precondition: `sweep` while the mandate is active (not revoked and not expired).
- Effect: refused.
- Error: `MANDATE_ACTIVE`

#### MANDATE-SWEEP-002 No third-party sweep during a principal change

- Precondition: `sweep` by a caller other than the principal while `proposed_principal` is set, in any state.
- Effect: refused, so nobody else can push the funds to the outgoing principal while the principal is moving to a new address (the case of a lost or compromised key). The principal itself is never held by this guard.
- Error: `PRINCIPAL_CHANGE_PENDING`

#### MANDATE-SWEEP-003 Grace period after expiry

- Precondition: `sweep` by a caller other than the principal while the mandate is not revoked and `now - expires_at < SWEEP_GRACE_SECONDS`. Here the mandate is expired, so `now >= expires_at` and the form cannot overflow.
- Effect: refused. A principal that let its mandate lapse has 30 days to extend it, withdraw or move its principal address before anyone else moves the funds. After revoke this guard does not apply, and the principal is never held by it.
- Error: `SWEEP_GRACE_PERIOD`

#### MANDATE-SWEEP-004 Nothing to sweep

- Precondition: `sweep` with `balance == 0`.
- Effect: refused.
- Error: `NOTHING_TO_SWEEP`

#### MANDATE-SWEEP-005 Sweep effects

- Precondition: `sweep` passed every check.
- Effect: transfer the whole balance to the principal and emit `swept { by, amount }`. `sweep` takes no destination argument, so the caller cannot redirect a unit. Its open trigger lets anyone return funds when the principal is unresponsive, at the caller's own fee cost; it does not help a principal that lost its key, which is why the guards exist. A failed transfer changes nothing and may be retried.
- Error: `TRANSFER_FAILED`

#### MANDATE-RECOVER-001 Token recovery is for other tokens

- Precondition: `recover_token(token, amount)` with `token == asset`.
- Effect: refused.
- Error: `WRONG_ASSET`

#### MANDATE-RECOVER-002 Token recovery amount

- Precondition: `recover_token(token, amount)` with `amount` outside `[1, I128_MAX]`.
- Effect: refused.
- Error: `AMOUNT_INVALID`

#### MANDATE-RECOVER-003 Token recovery effects

- Precondition: `recover_token` passed every check.
- Effect: transfer `amount` of `token` to the principal and emit `token_recovered { token, amount }`. Allowed in every state.
- Error: `TRANSFER_FAILED`

#### MANDATE-RECOVER-004 Token recovery never moves the settlement asset

- Precondition: after the `recover_token` transfer, `asset.balance(mandate)` differs from its value before the transfer.
- Effect: the whole call reverts. This holds even for a settlement asset reachable through a second token address, so token recovery can never bypass the free-balance cap of `withdraw`.
- Error: `WRONG_ASSET`

## Principal transfer

#### MANDATE-PRINCIPAL-001 Propose a new principal

- Precondition: `propose_principal(proposed)` where `proposed` is in `agents`, `executors` or `recovery`, or is `fee_address`.
- Effect: refused. The attester key overlap is a separate check (`MANDATE-ROLE-018`), and on Celo so is the zero address (`MANDATE-ROLE-017`). On success `proposed_principal = proposed` and `principal_proposed { current, proposed }` is emitted. A new proposal replaces an earlier one.
- Error: `ROLE_CONFLICT`

#### MANDATE-PRINCIPAL-002 Accept the transfer

- Precondition: `accept_principal()` from a caller other than `proposed_principal`.
- Effect: refused. `accept_principal` takes no caller argument: on Stellar it requires the authorization of `proposed_principal`, so another caller fails as a host authorization failure; on Celo `msg.sender` must be `proposed_principal`. On success the principal becomes the proposed address, the proposal clears, and `principal_transferred { previous, current }` is emitted.
- Error: `UNAUTHORIZED` (Celo); host authorization failure (Stellar)

#### MANDATE-PRINCIPAL-003 Cancel a proposal

- Precondition: `cancel_principal_proposal()` with no proposal set.
- Effect: refused. On success the proposal clears and `principal_proposal_cancelled { current, proposed }` is emitted. Without this call, a proposal to a wrong address could only be replaced, never withdrawn, and would hold every third-party sweep behind `MANDATE-SWEEP-002`.
- Error: `CONFIG_INVALID`

#### MANDATE-PRINCIPAL-004 The principal changes only in two steps

- Precondition: none.
- Effect: the principal changes only through a proposal by the current principal and an acceptance by the proposed address. Proposing, accepting and cancelling are allowed in every state, including after revoke and expiry.
- Error: none

#### MANDATE-PRINCIPAL-005 Accept needs a proposal

- Precondition: `accept_principal()` with no proposal set. Checked first, because with no proposal there is no address to authorize.
- Effect: refused on both chains with the same code.
- Error: `UNAUTHORIZED`

#### MANDATE-PRINCIPAL-006 Roles are re-checked at acceptance

- Precondition: `accept_principal()` while `proposed_principal` is in `agents`, `executors` or `recovery`, which can happen if it gained the role after the proposal.
- Effect: refused, so the new principal never holds a role `MANDATE-ROLE-002` or `MANDATE-ROLE-021` forbids. The principal removes the role, or cancels the proposal, first. The attester key overlap is `MANDATE-ROLE-018`.
- Error: `ROLE_CONFLICT`

## Take rate fee

#### MANDATE-FEE-001 One optional fee on top of the total

- Precondition: none.
- Effect: a request may carry one fee in its `fee` field, paid on top of `total` to `fee_address`; the recipient receives exactly `total`. The fee is not a leg: it does not count toward `MAX_LEGS`, the duplicate check, the allowlist, the attestation or the daily cap. It is not part of the attestation digest.
- Error: none

#### MANDATE-FEE-002 Fee cap

- Precondition: none.
- Effect: `fee_cap(total) = (total / 10000) * max_fee_bps + ((total % 10000) * max_fee_bps) / 10000`, with integer division. It equals `floor(total * max_fee_bps / 10000)` exactly but never forms `total * max_fee_bps`, which would overflow `i128` near `I128_MAX`. Rounding is down, in the principal's favour. Both chains compute this exact expression.
- Error: none

#### MANDATE-FEE-003 Fee within the cap

- Precondition: a request or an execute item with `fee > fee_cap(total)` under the `max_fee_bps` in force at that moment. With `max_fee_bps = 0` every non-zero fee fails.
- Effect: refused, so lowering the cap also stops queued payouts that carry a higher fee.
- Error: `FEE_EXCEEDS_CAP`

#### MANDATE-FEE-004 Which rules use the charge

- Precondition: none.
- Effect: mandate-level rules use `charged = total + fee`: budget, per-payment cap, approval threshold, free funds and balance, `pending`, `committed` and `spent`. Recipient-level rules use `total`: the leg sum and the daily cap. Approvers approve the stored payout, fee included.
- Error: none

#### MANDATE-FEE-005 Fee rate set at creation

- Precondition: a configuration with `max_fee_bps` outside `[0, MAX_FEE_BPS]`, checked as one of the field domains (`MANDATE-CONFIG-002`).
- Effect: refused. 0 means no fee: every payout must carry `fee = 0`.
- Error: `CONFIG_INVALID`

#### MANDATE-FEE-006 The fee cap can only go down

- Precondition: `lower_fee_cap(new_bps)` with `new_bps >= max_fee_bps`.
- Effect: refused. On success `max_fee_bps = new_bps` and `fee_cap_lowered { max_fee_bps }` is emitted. Allowed while paused and after expiry. Raising the cap needs a new mandate. Neither the operator nor the recovery set can change the cap or the fee address.
- Error: `CONFIG_INVALID`

#### MANDATE-FEE-007 The fee address is fixed

- Precondition: none.
- Effect: `fee_address` comes from the factory at creation and never changes. It is never the zero address, the principal or a role holder (`MANDATE-ROLE-008`).
- Error: none

#### MANDATE-FEE-008 Only execute pays the fee

- Precondition: none.
- Effect: the fee moves only in `execute`, last, after every leg, in the same atomic step, and at most once per payout. Request, approval, rejection, cancellation, lapse, revoke, withdraw and sweep never move it. If the fee transfer fails, the whole execute reverts.
- Error: `TRANSFER_FAILED`

## Events

#### MANDATE-EVENT-001 Exactly the specified events

- Precondition: none.
- Effect: every state change emits exactly the events listed for it below, in the order given, and nothing else. A failed call emits nothing.
- Error: none

#### MANDATE-EVENT-002 Canonical form

- Precondition: none.
- Effect: each event has a canonical form `{ name, fields }` with snake_case field names, which both chains' encodings decode to. Every destination in an event, including each entry of `legs`, is the canonical pair `{ address, tag }` with `tag` null when absent. Event shapes are frozen per spec version; a new field means a new event name in a new spec version.
- Error: none

#### MANDATE-EVENT-003 Funding has no mandate event

- Precondition: none.
- Effect: funding is the one balance change with no mandate event, because the mandate is not called; the asset's transfer event is the record.
- Error: none

### Event list

| Event | Fields |
| --- | --- |
| `mandate_created` (factory) | mandate, principal, asset, tenant_ref, spec_version, fee_address |
| `mandate_initialized` | principal, asset, recipient_mode, tenant_ref, agents, approvers, approvals_required, guardians, executors, recovery, recovery_threshold, limits, attester, country_policy_hash, max_fee_bps, fee_address |
| `payout_requested` | payout_id, agent, recipient_ref, total, fee, legs, task_ref, status, expires_at |
| `payout_request_rejected` | payout_id, agent, code |
| `payout_approval_added` | payout_id, approver, approvals, required |
| `payout_approved` | payout_id |
| `payout_rejected` | payout_id, by |
| `payout_cancelled` | payout_id, by, reason |
| `payout_executed` | payout_id, recipient_ref, total, fee, legs, day, spent |
| `fee_cap_lowered` | max_fee_bps |
| `paused`, `resumed` | by |
| `revoked` | by, frozen |
| `withdrawn` | to, amount |
| `swept` | by, amount |
| `token_recovered` | token, amount |
| `limits_updated` | total_budget, per_payment_cap, per_recipient_daily_cap, approval_threshold, payout_ttl_seconds, expires_at |
| `role_added`, `role_removed` | role, account |
| `approvals_required_set` | required |
| `recovery_threshold_set` | required, recovery_epoch |
| `recovery_vote_added` | kind, old, new, votes, required, recovery_epoch |
| `recovery_executed` | kind, old, new, by |
| `recipient_allowed`, `recipient_disallowed` | recipient_ref, destination |
| `attestation_policy_set` | attester, country_policy_hash, policy_epoch |
| `principal_proposed` | current, proposed |
| `principal_transferred` | previous, current |
| `principal_proposal_cancelled` | current, proposed |

`payout_executed` keeps its `legs` field: on Celo the token's transfer event carries no tag, so the mandate event is the only on-chain record of each leg's destination pair and its binding to the payout.

## Operations and check orders

Authorization comes first in every call. Every call that takes a `caller` address authorizes it as its first check (`MANDATE-ROLE-019`), including calls anyone may make (`cancel`, `cancel_batch`, `sweep`); on Stellar a missing signature is a host failure, not a mandate error, and on Celo the caller is `msg.sender`. Calls with no `caller` argument authorize a fixed address instead (`MANDATE-ROLE-015`, `MANDATE-CONFIG-003`, `MANDATE-PRINCIPAL-002`). `cancel` is the one call whose role decision comes after the lookup. Where a rule is marked Celo only, the check does not exist on Stellar and the next check follows.

### create (factory)

`create(config, salt) -> mandate address`

<!-- check-order: create -->
1. The caller is `config.principal` (MANDATE-CONFIG-003).
2. `config.asset` is supported by the factory (MANDATE-ASSET-001).
3. Every field is in its domain and the cross-field rules hold (MANDATE-CONFIG-002).
4. The attester is present exactly when the mode needs it, and well formed (MANDATE-CONFIG-005).
5. On Celo, neither the principal nor any role member is the zero address (MANDATE-ROLE-017).
6. The principal is in neither `agents` nor `executors` (MANDATE-ROLE-002).
7. `agents` and `approvers` are disjoint (MANDATE-ROLE-003).
8. `executors` and `approvers` are disjoint (MANDATE-ROLE-004).
9. `recovery` is disjoint from `agents` and `executors` (MANDATE-ROLE-005).
10. `agents` and `executors` are disjoint (MANDATE-ROLE-020).
11. The principal is not a recovery approver (MANDATE-ROLE-021).
12. The fee address is neither the principal nor in any role set (MANDATE-ROLE-008).
13. The attester's key matches neither the principal, the fee address nor any role account (MANDATE-ROLE-018).
14. No role set exceeds its bound (MANDATE-ROLE-007).
15. `approvals_required` fits the approvers (MANDATE-APPROVAL-002).
16. `recovery_threshold` fits the recovery set (MANDATE-RECOVERYSET-002).
<!-- /check-order -->

Checks 3 to 16 run in the factory before it deploys, and again in the constructor in the same order (`MANDATE-CONFIG-004`). `max_fee_bps` is part of check 3. Effects: `MANDATE-CONFIG-006`.

### request_payout

`request_payout(caller, req) -> status`, where `req = { payout_id, recipient_ref, total, fee, legs, deadline, task_ref, attestations }` and each attestation is `{ valid_until, policy_epoch, signature }`.

<!-- check-order: request_payout -->
1. The caller authorizes the call (MANDATE-ROLE-019).
2. The caller is an agent (MANDATE-ROLE-010).
3. The mandate is not revoked (MANDATE-REVOKE-002).
4. The mandate is not expired (MANDATE-EXPIRY-002).
5. The mandate is not paused (MANDATE-PAUSE-001).
6. The payout ID is unused (MANDATE-PAYOUT-002).
7. The deadline has not passed (MANDATE-PAYOUT-003).
8. Total and fee are in their domains (MANDATE-AMOUNT-004).
9. The leg count is 1 to `MAX_LEGS` (MANDATE-SPLIT-001).
10. Every leg amount is in its domain (MANDATE-SPLIT-002).
11. No leg pays the zero address (MANDATE-SPLIT-003).
12. Every leg tag is canonical (MANDATE-SPLIT-004).
13. No leg pays the fee address (MANDATE-SPLIT-008).
14. No two legs share a destination pair (MANDATE-SPLIT-005).
15. The legs sum to the total (MANDATE-SPLIT-006).
16. In `ALLOWLIST` mode, there are no attestations (MANDATE-RECIPIENT-003).
17. In `ATTESTED` and `BOTH` modes, there is one attestation per leg (MANDATE-RECIPIENT-004).
18. For each leg in order, in `ALLOWLIST` and `BOTH` modes, the destination pair is on the allowlist (MANDATE-RECIPIENT-005).
19. For the same leg, in `ATTESTED` and `BOTH` modes, the attestation is not expired (MANDATE-RECIPIENT-006).
20. For the same leg, the attestation window is at most `MAX_ATTESTATION_VALIDITY` (MANDATE-RECIPIENT-007).
21. For the same leg, the signature is 64 bytes (MANDATE-RECIPIENT-008).
22. For the same leg, the attestation names the current policy epoch (MANDATE-RECIPIENT-019).
23. For the same leg, the signature is the attester's ed25519 signature over the attestation message (MANDATE-RECIPIENT-012).
24. The fee is within the fee cap (MANDATE-FEE-003).
25. The charge is within the per-payment cap (MANDATE-PAYMENT-001).
26. The charge is within the remaining budget (MANDATE-BUDGET-001).
27. The total is within the recipient's daily headroom (MANDATE-DAILY-002).
28. The charge is within the free balance (MANDATE-FUND-002).
<!-- /check-order -->

Checks 18 to 23 run for one leg before the next leg starts. Effects: `MANDATE-PAYOUT-007` and `MANDATE-APPROVAL-001`.

### request_payouts

`request_payouts(caller, reqs) -> ItemResult[]`

<!-- check-order: request_payouts -->
1. The caller authorizes the call (MANDATE-ROLE-019).
2. The caller is an agent (MANDATE-ROLE-010).
3. The mandate is not revoked (MANDATE-REVOKE-002).
4. The mandate is not expired (MANDATE-EXPIRY-002).
5. The mandate is not paused (MANDATE-PAUSE-001).
6. The batch has 1 to `MAX_BATCH` items (MANDATE-BATCH-001).
7. Each item runs `request_payout` checks 6 to 28 and its effects, per item (MANDATE-BATCH-002).
<!-- /check-order -->

### approve

`approve(caller, payout_id) -> status`

<!-- check-order: approve -->
1. The caller authorizes the call (MANDATE-ROLE-019).
2. The caller is an approver (MANDATE-ROLE-011).
3. The mandate is not revoked (MANDATE-REVOKE-002).
4. The mandate is not expired (MANDATE-EXPIRY-002).
5. The mandate is not paused (MANDATE-PAUSE-001).
6. The payout exists (MANDATE-PAYOUT-004).
7. The payout is PENDING (MANDATE-PAYOUT-005).
8. The payout has not lapsed (MANDATE-PAYOUT-006).
9. The payout's agent is still the same agent (MANDATE-PAYOUT-014).
10. The caller has not approved already, unless the requirement is already met (MANDATE-APPROVAL-003).
<!-- /check-order -->

Effects: `MANDATE-APPROVAL-004`.

### reject

`reject(caller, payout_id)`. Allowed while paused and after expiry.

<!-- check-order: reject -->
1. The caller authorizes the call (MANDATE-ROLE-019).
2. The caller is an approver or the principal (MANDATE-ROLE-012).
3. The mandate is not revoked (MANDATE-REVOKE-002).
4. The payout exists (MANDATE-PAYOUT-004).
5. The payout is PENDING (MANDATE-PAYOUT-005).
<!-- /check-order -->

Effects: `MANDATE-APPROVAL-005`.

### cancel

`cancel(caller, payout_id)`

<!-- check-order: cancel -->
1. The caller authorizes the call (MANDATE-ROLE-019).
2. The mandate is not revoked (MANDATE-REVOKE-002).
3. The payout exists (MANDATE-PAYOUT-004).
4. The payout is PENDING or APPROVED (MANDATE-PAYOUT-005).
5. The caller may cancel it for reason 1, 2 or 3 (MANDATE-PAYOUT-010).
<!-- /check-order -->

Effects: `MANDATE-PAYOUT-011`.

### cancel_batch

`cancel_batch(caller, ids) -> ItemResult[]`

<!-- check-order: cancel_batch -->
1. The caller authorizes the call (MANDATE-ROLE-019).
2. The mandate is not revoked (MANDATE-REVOKE-002).
3. The batch has 1 to `MAX_BATCH` items, then each item runs `cancel` checks 3 to 5 and its effects, never reverting the call (MANDATE-BATCH-003).
<!-- /check-order -->

### execute

`execute(caller, payout_id)`

<!-- check-order: execute -->
1. The caller authorizes the call (MANDATE-ROLE-019).
2. The caller is the principal, an agent or an executor (MANDATE-ROLE-013).
3. The mandate is not revoked (MANDATE-REVOKE-002).
4. The mandate is not expired (MANDATE-EXPIRY-002).
5. The mandate is not paused (MANDATE-PAUSE-001).
6. The payout exists (MANDATE-PAYOUT-004).
7. The payout is APPROVED (MANDATE-PAYOUT-005).
8. The payout has not lapsed (MANDATE-PAYOUT-006).
9. The payout's agent is still the same agent (MANDATE-PAYOUT-014).
10. The fee is within the current fee cap (MANDATE-FEE-003).
11. The charge is within the current per-payment cap (MANDATE-PAYMENT-002).
12. In `ALLOWLIST` and `BOTH` modes, every leg's destination pair is still on the allowlist (MANDATE-RECIPIENT-014).
13. In `ATTESTED` and `BOTH` modes, the policy epoch is unchanged (MANDATE-RECIPIENT-015).
14. The total is within the recipient's daily headroom against executed usage (MANDATE-DAILY-003).
15. The charge is within the balance (MANDATE-FUND-003).
<!-- /check-order -->

The fee cap comes before the per-payment cap here as at request, so both calls report a fee problem the same way. Effects: `MANDATE-PAYOUT-012`, with every transfer under `MANDATE-PAYOUT-013` and the fee under `MANDATE-FEE-008`.

### execute_batch

`execute_batch(caller, ids)`

<!-- check-order: execute_batch -->
1. The caller authorizes the call (MANDATE-ROLE-019).
2. The caller is the principal, an agent or an executor (MANDATE-ROLE-013).
3. The mandate is not revoked (MANDATE-REVOKE-002).
4. The mandate is not expired (MANDATE-EXPIRY-002).
5. The mandate is not paused (MANDATE-PAUSE-001).
6. The batch has 1 to `MAX_BATCH` items (MANDATE-BATCH-004).
7. Each item in order runs `execute` checks 6 to 15 against the state left by earlier items (MANDATE-BATCH-005).
8. For the same item, the running transfer count stays within `MAX_BATCH_TRANSFERS` (MANDATE-BATCH-006).
<!-- /check-order -->

Checks 7 and 8 and the item's effects, its transfers included, run for one item before the next item's checks start.

### pause

`pause(caller)`

<!-- check-order: pause -->
1. The caller authorizes the call (MANDATE-ROLE-019).
2. The caller is the principal, an approver, a guardian or a recovery approver (MANDATE-ROLE-014).
3. The mandate is not revoked (MANDATE-REVOKE-002).
4. The mandate is not already paused (MANDATE-PAUSE-002).
<!-- /check-order -->

### resume

`resume()`

<!-- check-order: resume -->
1. The caller is the principal (MANDATE-ROLE-015).
2. The mandate is not revoked (MANDATE-REVOKE-002).
3. The mandate is paused (MANDATE-PAUSE-003).
<!-- /check-order -->

### revoke

`revoke() -> frozen`. Allowed while paused or expired.

<!-- check-order: revoke -->
1. The caller is the principal (MANDATE-ROLE-015).
2. The mandate is not already revoked (MANDATE-REVOKE-002).
<!-- /check-order -->

Effects: `MANDATE-REVOKE-001`.

### withdraw

`withdraw(amount)`. Allowed in every state.

<!-- check-order: withdraw -->
1. The caller is the principal (MANDATE-ROLE-015).
2. The amount is in its domain (MANDATE-WITHDRAW-002).
3. The amount is within the free balance (MANDATE-WITHDRAW-003).
<!-- /check-order -->

Effects: `MANDATE-WITHDRAW-004`.

### sweep

`sweep(caller) -> amount`. Anyone may call it.

<!-- check-order: sweep -->
1. The caller authorizes the call (MANDATE-ROLE-019).
2. The mandate is not active (MANDATE-SWEEP-001).
3. A caller other than the principal is refused while a principal change is proposed (MANDATE-SWEEP-002).
4. A caller other than the principal is refused within the grace after expiry, unless revoked (MANDATE-SWEEP-003).
5. The balance is not zero (MANDATE-SWEEP-004).
<!-- /check-order -->

Effects: `MANDATE-SWEEP-005`.

### recover_token

`recover_token(token, amount)`. Allowed in every state.

<!-- check-order: recover_token -->
1. The caller is the principal (MANDATE-ROLE-015).
2. The token is not the settlement asset (MANDATE-RECOVER-001).
3. The amount is in its domain (MANDATE-RECOVER-002).
<!-- /check-order -->

Effects: `MANDATE-RECOVER-003`, then the settlement balance check of `MANDATE-RECOVER-004`.

### update_limits

`update_limits(limits)`

<!-- check-order: update_limits -->
1. The caller is the principal (MANDATE-ROLE-015).
2. The mandate is not revoked (MANDATE-REVOKE-002).
3. Every field is in its domain, the cross-field rules hold and `expires_at > now` (MANDATE-CONFIG-007).
4. With no approvers, the new threshold is above the new per-payment cap (MANDATE-APPROVAL-009).
5. The new budget is not below what is reserved and spent (MANDATE-BUDGET-002).
6. Reactivating an expired mandate needs `balance >= pending + committed` (MANDATE-FUND-004).
<!-- /check-order -->

### add_role

`add_role(role, account)`

<!-- check-order: add_role -->
1. The caller is the principal (MANDATE-ROLE-015).
2. The mandate is not revoked (MANDATE-REVOKE-002).
3. The role code is known and the account is not already in the set (MANDATE-ROLE-016).
4. On Celo, the account is not the zero address (MANDATE-ROLE-017).
5. The account would not break role separation (MANDATE-ROLE-009).
6. The account's key is not the attester's key (MANDATE-ROLE-018).
7. The set would not exceed its bound (MANDATE-ROLE-007).
<!-- /check-order -->

Effects: add the account, emit `role_added { role, account }`; for an agent, increase `agent_epoch` and store the new value with it (`MANDATE-PAYOUT-014`); apply `MANDATE-APPROVAL-007` when it is the first approver, and increase `recovery_epoch` when the role is agent or recovery (`MANDATE-RECOVERYSET-003`).

### remove_role

`remove_role(role, account)`

<!-- check-order: remove_role -->
1. The caller is the principal (MANDATE-ROLE-015).
2. The mandate is not revoked (MANDATE-REVOKE-002).
3. The role code is known and the account is in the set (MANDATE-ROLE-016).
4. Removing one approver among several keeps `approvals_required` reachable (MANDATE-APPROVAL-006).
5. Removing the last approver needs the threshold above the per-payment cap (MANDATE-APPROVAL-008).
6. Removing a recovery approver keeps the threshold within the set size (MANDATE-RECOVERYSET-002).
<!-- /check-order -->

Effects: remove the account, emit `role_removed { role, account }`, apply `MANDATE-APPROVAL-008` when it was the last approver, and increase `recovery_epoch` when the role is agent or recovery.

### set_approvals_required

`set_approvals_required(n)`

<!-- check-order: set_approvals_required -->
1. The caller is the principal (MANDATE-ROLE-015).
2. The mandate is not revoked (MANDATE-REVOKE-002).
3. `n` fits the approvers (MANDATE-APPROVAL-002).
<!-- /check-order -->

### set_recovery_threshold

`set_recovery_threshold(n)`

<!-- check-order: set_recovery_threshold -->
1. The caller is the principal (MANDATE-ROLE-015).
2. The mandate is not revoked (MANDATE-REVOKE-002).
3. `n` fits the recovery set (MANDATE-RECOVERYSET-002).
<!-- /check-order -->

Effects: set the threshold, increase `recovery_epoch`, emit `recovery_threshold_set { required, recovery_epoch }`.

### allow_recipients

`allow_recipients(entries)`

<!-- check-order: allow_recipients -->
1. The caller is the principal (MANDATE-ROLE-015).
2. The mandate is not revoked (MANDATE-REVOKE-002).
3. The mode is not `ATTESTED` (MANDATE-RECIPIENT-016).
4. There are 1 to `MAX_BATCH` entries (MANDATE-BATCH-007).
5. No entry has the zero address, a non-canonical tag or the fee address (MANDATE-RECIPIENT-016).
6. No entry is already present, counting duplicates within the call (MANDATE-RECIPIENT-016).
<!-- /check-order -->

### disallow_recipients

`disallow_recipients(entries)`

<!-- check-order: disallow_recipients -->
1. The caller is the principal (MANDATE-ROLE-015).
2. The mandate is not revoked (MANDATE-REVOKE-002).
3. The mode is not `ATTESTED` (MANDATE-RECIPIENT-016).
4. There are 1 to `MAX_BATCH` entries (MANDATE-BATCH-007).
5. No entry has the zero address, a non-canonical tag or the fee address (MANDATE-RECIPIENT-016).
6. Every entry is present, counting duplicates within the call (MANDATE-RECIPIENT-016).
<!-- /check-order -->

### set_attestation_policy

`set_attestation_policy(attester, country_policy_hash)`

<!-- check-order: set_attestation_policy -->
1. The caller is the principal (MANDATE-ROLE-015).
2. The mandate is not revoked (MANDATE-REVOKE-002).
3. The mode is not `ALLOWLIST` and the key is well formed (MANDATE-CONFIG-008).
4. The key matches no role account (MANDATE-ROLE-018).
<!-- /check-order -->

### propose_principal

`propose_principal(proposed)`. Allowed in every state.

<!-- check-order: propose_principal -->
1. The caller is the principal (MANDATE-ROLE-015).
2. On Celo, the proposed address is not the zero address (MANDATE-ROLE-017).
3. The proposed address holds no conflicting role (MANDATE-PRINCIPAL-001).
4. The proposed address's key is not the attester's key (MANDATE-ROLE-018).
<!-- /check-order -->

### accept_principal

`accept_principal()`. Allowed in every state.

<!-- check-order: accept_principal -->
1. A proposal is set (MANDATE-PRINCIPAL-005).
2. The caller is the proposed principal (MANDATE-PRINCIPAL-002).
3. The proposed principal holds no conflicting role (MANDATE-PRINCIPAL-006).
4. The proposed principal's key is not the attester's key (MANDATE-ROLE-018).
<!-- /check-order -->

### cancel_principal_proposal

`cancel_principal_proposal()`. Allowed in every state.

<!-- check-order: cancel_principal_proposal -->
1. The caller is the principal (MANDATE-ROLE-015).
2. A proposal is set (MANDATE-PRINCIPAL-003).
<!-- /check-order -->

### lower_fee_cap

`lower_fee_cap(new_bps)`. Allowed while paused and after expiry.

<!-- check-order: lower_fee_cap -->
1. The caller is the principal (MANDATE-ROLE-015).
2. The mandate is not revoked (MANDATE-REVOKE-002).
3. `new_bps` is below the current cap (MANDATE-FEE-006).
<!-- /check-order -->

### recovery_revoke

`recovery_revoke(caller) -> status`. Allowed while paused and after expiry.

<!-- check-order: recovery_revoke -->
1. The caller authorizes the call (MANDATE-ROLE-019).
2. The caller is a recovery approver (MANDATE-RECOVERYSET-004).
3. The mandate is not revoked (MANDATE-REVOKE-002).
4. The caller has not voted for this action in this epoch (MANDATE-RECOVERYSET-007).
<!-- /check-order -->

Effects: `MANDATE-RECOVERYSET-008`.

### recovery_rotate_agent

`recovery_rotate_agent(caller, old, new) -> status`, where `new` is optional. Allowed while paused and after expiry.

<!-- check-order: recovery_rotate_agent -->
1. The caller authorizes the call (MANDATE-ROLE-019).
2. The caller is a recovery approver (MANDATE-RECOVERYSET-004).
3. The mandate is not revoked (MANDATE-REVOKE-002).
4. `old` is a current agent and `new`, if given, is not (MANDATE-RECOVERYSET-005).
5. `new`, if given, holds no conflicting role (MANDATE-RECOVERYSET-006).
6. The key of `new`, if given, is not the attester's key (MANDATE-ROLE-018).
7. The caller has not voted for this action in this epoch (MANDATE-RECOVERYSET-007).
<!-- /check-order -->

Effects: `MANDATE-RECOVERYSET-008`.

### Views

`version`, `config`, `state`, `payout`, `daily_used` and `is_allowed` change nothing and have no checks (`MANDATE-CONFIG-011`).

## Invariants

The vector runner checks these after every step, and each chain's invariant tests check them under random call sequences.

- I1. `spent + pending + committed <= total_budget`.
- I2. While active, `balance >= pending + committed`, except when the asset itself freezes or removes the mandate's balance. Reactivating an expired mandate keeps it (`MANDATE-FUND-004`).
- I3. For every payout, the legs sum to `total`, and `total + fee <= per_payment_cap` at request and at execute.
- I4. For every recipient and UTC day, the totals executed that day never exceed the cap in force at each execution.
- I5. The settlement asset leaves the mandate only through `execute`, to destinations bound to the payout's recipient (including recipient-bound forwarders and partner deposit addresses verified off chain under `CHAINOPS-DELIVERY-001` and `CHAINOPS-DELIVERY-002`) plus each executed payout's fee to `fee_address`, and through `withdraw` and `sweep` to the principal. `withdraw` and `sweep` never take a destination argument; this is checked by an ABI test on Celo, a contract interface test on Stellar, and vectors that call them from every role and confirm that only the principal's balance rises.
- I6. A payout ID reaches EXECUTED at most once, terminal statuses never change, and an ID is never reused.
- I7. `spent` never decreases and `revoked` never clears.
- I8. A payout with `charged >= approval_threshold` at request reaches APPROVED only through approvals from distinct current approvers, none of whom is an agent or an executor.
- I9. Every state change emits exactly its specified events, and failed calls emit nothing.
- I10. `pending` equals the sum of the charges of PENDING payouts and `committed` the sum of the charges of APPROVED payouts, until revoke sets both to zero.
- I11. Recovery actions never transfer funds, and never change a destination, the allowlist, the attestation policy, a limit, the principal, the approvers, the guardians, the executors or the recovery set. A recovery revoke freezes the balance for the principal like any revoke.
- I12. For every payout, `fee <= fee_cap(total)` under the `max_fee_bps` in force at request and at execute; `max_fee_bps <= MAX_FEE_BPS` always and never increases; the fee is transferred only to `fee_address`, only by `execute`, and exactly once per executed payout; rejected, cancelled and lapsed payouts pay no fee.

## Off-chain obligations

These rules bind the operator's services, not the contracts. They are listed here because the contracts' safety depends on them, they are tested in the services, and they are excluded from vector coverage.

#### CHAINOPS-XCHAIN-001 Fail over only after a provable death

- Off chain: a payout attempt may be submitted on a second chain only once it is provably dead on the first: a confirmed REJECTED or CANCELLED tombstone, or finalized chain time at or after its deadline with no record. On-chain guarantees hold per chain instance; a combined cap across chains is enforced off chain.

#### CHAINOPS-DELIVERY-001 Forwarders are bound by recomputation

- Off chain: a forwarder may be allowlisted or attested for a `recipient_ref` only if recomputing its address from a trusted forwarder factory and its parameters shows its recipient is a wallet already bound to that `recipient_ref`.

#### CHAINOPS-DELIVERY-002 Partner deposit addresses need partner confirmation

- Off chain: a partner deposit address may be attested for a `recipient_ref` only after the partner confirms it belongs to that recipient's order. Pooled addresses are never allowlisted on Celo, tag or no tag, because a Celo token transfer carries no tag.

#### CHAINOPS-DELIVERY-003 Only payable Stellar destinations are bindable

- Off chain: a Stellar destination that needs a text or hash memo cannot be paid from a contract and is rejected at onboarding. Only `G`, `C` and `M` (numeric id) destinations are bindable.

#### CHAINOPS-DELIVERY-004 Providers are certified before use

- Off chain: an exchange, anchor or off-ramp address is bindable as a leg destination only after the provider confirms in writing that it credits contract-originated asset transfers to its `M` or `G` plus id addresses, a testnet certification payment from a real mandate is credited end to end for that corridor, and, for anchors, the anchor gives every receiver a fixed tag, so no pooled anchor leg ever runs.

#### CHAINOPS-SUBMIT-001 No memo on contract calls

- Off chain: a transaction that invokes a mandate on Stellar never carries a memo; the network rejects it. Pooled destinations are addressed only through the leg's tag.

#### CHAINOPS-SUBMIT-002 Submission discipline on Stellar

- Off chain: every Stellar mandate call is built from a fresh simulation, never a hand-built footprint. Calls that may catch a transfer failure add at least 512 bytes to the simulated write bytes and the matching resource fee; since revoke moves no funds no current call does, so the padding applies to none today and stays as the rule for any future call. `ResourceLimitExceeded` or `EntryArchived` results are re-simulated and resubmitted with bounded retries, never read as a mandate rejection. Only the `getTransaction` result proves a transaction landed. After any restore, the TTL of the instance and of the asset contract's balance entry for the mandate is extended (the balance entry has its own TTL, so a quiet mandate's money is never archived by surprise); a restored open payout record is extended only as far as `MANDATE-PAYOUT-016` allows, and a restored tombstone is not extended at all. Tested end to end on testnet: a revoke for a principal without a trustline lands with no padding, `frozen` equals the balance, and the funds are withdrawn once the trustline exists.

#### CHAINOPS-SUBMIT-003 Simulate attested requests before submitting

- Off chain: every request that carries attestations is simulated against the current ledger before it is submitted, and a simulation that traps is never submitted. When a batch simulation traps, the trapping item is isolated by bisection, removed and reported, and the rest is simulated again before submitting. When the mandate's `policy_epoch` changes, every attestation issued under the old epoch is discarded and the affected legs are attested again. So a stale or wrong attestation costs no fee and never aborts a batch on chain.

#### CHAINOPS-SUBMIT-004 Cancel lapsed payouts while their records are live

- Off chain: the operator cancels every lapsed payout (`MANDATE-PAYOUT-006`; anyone may cancel one, `MANDATE-PAYOUT-010`) before its record archives, timing the deadline from the record's `liveUntilLedgerSeq` read with `getLedgerEntries`, not from `expires_at` plus the margin, because faster ledgers can archive a record early (`MANDATE-PAYOUT-016`). Its `pending` or `committed` amount is then released without restoring the record; on a quiet mandate the cancel may still need to restore the contract instance or code. A missed window costs only a restore fee, because an archived record still counts as present (`MANDATE-PAYOUT-009`).

#### CHAINOPS-CREATE-001 The pause-only guardian is on by default

- Off chain: every mandate created through the operator's dashboard or API includes the operator's pause-only guardian by default, shown plainly. The principal can opt out at creation or remove it later, and it is never re-added after removal.

#### CHAINOPS-CREATE-002 Large mandates need more than one key

- Off chain: the dashboard always warns about a single-key principal. When a mandate's total budget, summed over its chain instances at 1 USD per USDC or USDm unit, exceeds 50,000 USD, creation and any budget increase crossing that line are refused unless the principal is a multisig with a signing threshold of at least 2, read on chain, or the instance has a recovery approver set with `recovery_threshold` of at least 2. A Stellar contract wallet counts as a multisig only if its wallet type is on the operator's list of types with a threshold reader. Because the contract cannot price in USD, a principal who raises the budget directly is no longer served by the operator's router and executor until it complies.

#### CHAINOPS-CREATE-003 Custody guard for operator-held agent keys

- Off chain: a mandate instance where the operator holds an agent key must use recipient mode `ALLOWLIST` or `BOTH`, so a principal-signed allowlist always binds destinations and no single compromise of the operator's agent and attester keys can direct funds. Creation with an operator-held agent key and `ATTESTED` mode is refused; preparing `add_role` for an operator-held agent key on an `ATTESTED` instance is refused (the recipient mode is immutable); and the operator's agent signer refuses to sign for an `ATTESTED` instance.

#### CHAINOPS-CREATE-004 The attester key signs for no role account

- Off chain: the attester service refuses to sign for a mandate while its key is a signer, of any weight, on the principal, `fee_address` or any role account, including the signers of a contract (`C`) smart wallet read through the wallet's own interface. It checks at mandate creation, on every role change and on every signer change of those accounts it observes, and again before each signature. When it cannot read the full signer set of a smart wallet principal or role account, it refuses to sign: a failed or partial authority read is never treated as clean.

#### CHAINOPS-CREATE-005 Attester key form

- Off chain: when an attester key is configured or rotated, the operator checks that it decompresses to a canonical ed25519 point that is not of small order, and refuses it otherwise; it may also require the key to prove possession by signing a configuration challenge. The contract only checks its length and that it is not all zero (`MANDATE-CONFIG-005`).

#### CHAINOPS-SWEEP-001 Every sweep is booked once

- Off chain: every sweep is booked once, keyed by the `swept` event's `(chain, tx_hash, event_index)` under a unique constraint, whoever triggered it.

#### CHAINOPS-SWEEP-002 Sweep notifications are grouped

- Off chain: sweep notifications are grouped, one message per principal per sweep run or per day, never one per mandate.

#### CHAINOPS-SWEEP-003 Third-party funding of closed mandates alerts

- Off chain: inflows are attributed to their sender. A transfer into a revoked or expired mandate from anyone other than the principal or its own funding account alerts the operator and the principal before any sweep, because a sweep would pass third-party money to the principal.

#### CHAINOPS-SWEEP-004 The operator's sweeper respects the guards

- Off chain: the operator's sweeper skips mandates within `SWEEP_GRACE_SECONDS` of expiry (unless revoked) and mandates with a pending principal change, so it never submits a sweep the contract would refuse.

#### CHAINOPS-FEE-001 The fee is shown and reconciled

- Off chain: every receipt shows the fee as its own line (amount, currency and the rate applied) next to the amount the recipient received, and principal views show `charged`. Every executed fee is recorded in the off-chain ledger and reconciled against `payout_executed` and the fee transfer.

#### CHAINOPS-FEE-002 The request builder sets the fee

- Off chain: the operator's request builder sets `fee` from the principal's agreed rate with the same `fee_cap` expression, never above `max_fee_bps`, and 0 for at-cost internal payouts. Requests built elsewhere may carry a lower fee; the contract only stops overcharging, so underpayment is found in reconciliation and handled commercially.

#### CHAINOPS-FEE-003 The fee address is a dedicated multisig

- Off chain: the fee address on each chain is a multisig with a signing threshold of at least 2 (on Stellar an account or contract wallet with a USDC trustline; on Celo a Safe), monitored and used for nothing else. Changing it means a new factory for new mandates; existing mandates keep their address.

#### CHAINOPS-FEE-004 The fee on a refunded part is returned

- Off chain: when an anchor refunds a leg to the principal, the fee on the refunded part was not earned. The operator returns `ceil(fee * refunded_amount / total)`, capped at `fee`, from the fee address to the principal's own address on the same chain within 7 days, and shows it on the receipt. A resend is a new payout and carries its own fee.

#### CHAINOPS-FEE-005 A fee address that cannot receive

- Off chain: if the fee address cannot receive, every execute with a non-zero fee reverts. The operator cancels the affected payouts, re-requests them with `fee = 0`, and invoices that fee. Principal funds are never at risk; only liveness is affected until the address is fixed.

## Rule index

| ID | Title |
| --- | --- |
| `MANDATE-ROLE-001` | Authority comes from role addresses only |
| `MANDATE-ROLE-002` | The principal is never an agent or an executor |
| `MANDATE-ROLE-003` | Agents and approvers are disjoint |
| `MANDATE-ROLE-004` | Executors and approvers are disjoint |
| `MANDATE-ROLE-005` | Recovery approvers are disjoint from agents and executors |
| `MANDATE-ROLE-006` | Allowed overlaps |
| `MANDATE-ROLE-007` | Role sets are bounded |
| `MANDATE-ROLE-008` | The fee address holds no role |
| `MANDATE-ROLE-009` | Role conflict check on add |
| `MANDATE-ROLE-010` | Agent-only calls |
| `MANDATE-ROLE-011` | Approver-only calls |
| `MANDATE-ROLE-012` | Who may reject |
| `MANDATE-ROLE-013` | Who may execute |
| `MANDATE-ROLE-014` | Who may pause |
| `MANDATE-ROLE-015` | Principal-only calls |
| `MANDATE-ROLE-016` | Role change validation |
| `MANDATE-ROLE-017` | No zero address as principal or role member |
| `MANDATE-ROLE-018` | The attester's key holds no role |
| `MANDATE-ROLE-019` | Every caller argument is authorized first |
| `MANDATE-ROLE-020` | Agents and executors are disjoint |
| `MANDATE-ROLE-021` | The principal is not a recovery approver |
| `MANDATE-RECOVERYSET-001` | Only the principal changes the set |
| `MANDATE-RECOVERYSET-002` | Threshold bounds |
| `MANDATE-RECOVERYSET-003` | Recovery epoch |
| `MANDATE-RECOVERYSET-004` | Recovery-only calls |
| `MANDATE-RECOVERYSET-005` | Rotation targets an existing agent |
| `MANDATE-RECOVERYSET-006` | Rotation respects the role rules |
| `MANDATE-RECOVERYSET-007` | One vote per member per action and epoch |
| `MANDATE-RECOVERYSET-008` | Voting and execution |
| `MANDATE-RECOVERYSET-009` | Recovery powers are limited |
| `MANDATE-CONFIG-001` | Immutable fields |
| `MANDATE-CONFIG-002` | Field domains and cross-field rules |
| `MANDATE-CONFIG-003` | Caller of create is the principal |
| `MANDATE-CONFIG-004` | The factory validates before it deploys |
| `MANDATE-CONFIG-005` | Attester presence and form |
| `MANDATE-CONFIG-006` | Deterministic address and creation events |
| `MANDATE-CONFIG-007` | update_limits |
| `MANDATE-CONFIG-008` | set_attestation_policy |
| `MANDATE-CONFIG-009` | Configuration changes are atomic |
| `MANDATE-CONFIG-010` | Derived values |
| `MANDATE-CONFIG-011` | Views |
| `MANDATE-PAYOUT-001` | Tombstones keep every ID used forever |
| `MANDATE-PAYOUT-002` | A used ID is refused |
| `MANDATE-PAYOUT-003` | Request deadline |
| `MANDATE-PAYOUT-004` | Unknown payout |
| `MANDATE-PAYOUT-005` | Status the call accepts |
| `MANDATE-PAYOUT-006` | Lapsed payout |
| `MANDATE-PAYOUT-007` | Request effects |
| `MANDATE-PAYOUT-008` | IDs are opaque to the contract |
| `MANDATE-PAYOUT-009` | Archived entries are never absent |
| `MANDATE-PAYOUT-010` | Who may cancel |
| `MANDATE-PAYOUT-011` | Cancel effects |
| `MANDATE-PAYOUT-012` | Execute effects |
| `MANDATE-PAYOUT-013` | Transfers out are wrapped |
| `MANDATE-PAYOUT-014` | The requesting agent is still the same agent |
| `MANDATE-PAYOUT-015` | No nested call into the mandate during a transfer |
| `MANDATE-PAYOUT-016` | Payout record lifetime on Stellar |
| `MANDATE-AMOUNT-001` | Amounts are integer smallest units |
| `MANDATE-AMOUNT-002` | Amount domain |
| `MANDATE-AMOUNT-003` | Only addition, subtraction and comparison |
| `MANDATE-AMOUNT-004` | Request amounts |
| `MANDATE-AMOUNT-005` | Exact formulas |
| `MANDATE-AMOUNT-006` | Unreachable arithmetic |
| `MANDATE-EXPIRY-001` | Time |
| `MANDATE-EXPIRY-002` | Expired mandates refuse new work |
| `MANDATE-EXPIRY-003` | Payout time to live |
| `MANDATE-EXPIRY-004` | Attested payouts expire with their attestations |
| `MANDATE-RECIPIENT-001` | A destination is an address and an optional tag |
| `MANDATE-RECIPIENT-002` | Destination form on each chain |
| `MANDATE-RECIPIENT-003` | Allowlist mode takes no attestations |
| `MANDATE-RECIPIENT-004` | One attestation per leg |
| `MANDATE-RECIPIENT-005` | Leg destination is on the allowlist |
| `MANDATE-RECIPIENT-006` | Attestation not expired |
| `MANDATE-RECIPIENT-007` | Attestation window |
| `MANDATE-RECIPIENT-008` | Signature length |
| `MANDATE-RECIPIENT-009` | Retired: recovery byte |
| `MANDATE-RECIPIENT-010` | Retired: r in range |
| `MANDATE-RECIPIENT-011` | Retired: low s |
| `MANDATE-RECIPIENT-012` | Signed by the attester |
| `MANDATE-RECIPIENT-013` | Retired: unrecoverable signatures |
| `MANDATE-RECIPIENT-014` | Allowlist still holds at execute |
| `MANDATE-RECIPIENT-015` | Attestation policy unchanged at execute |
| `MANDATE-RECIPIENT-016` | Allowlist entry validation |
| `MANDATE-RECIPIENT-017` | Policy epoch |
| `MANDATE-RECIPIENT-018` | Choosing the recipient mode |
| `MANDATE-RECIPIENT-019` | Attestation names the current policy epoch |
| `MANDATE-SPLIT-001` | Leg count |
| `MANDATE-SPLIT-002` | Leg amount |
| `MANDATE-SPLIT-003` | No zero-address leg |
| `MANDATE-SPLIT-004` | Canonical tag on legs |
| `MANDATE-SPLIT-005` | No duplicate destination |
| `MANDATE-SPLIT-006` | Legs sum to the total |
| `MANDATE-SPLIT-007` | Limits apply to the payout, not its legs |
| `MANDATE-SPLIT-008` | No leg to the fee address |
| `MANDATE-FUND-001` | Funding is a plain transfer |
| `MANDATE-FUND-002` | Free funds at request |
| `MANDATE-FUND-003` | Balance at execute |
| `MANDATE-FUND-004` | Reactivation needs the reservations covered |
| `MANDATE-DUST-001` | Dust is part of the balance |
| `MANDATE-ASSET-001` | Supported asset |
| `MANDATE-ASSET-002` | Other tokens are never counted |
| `MANDATE-BUDGET-001` | Budget at request |
| `MANDATE-BUDGET-002` | Budget never below what is reserved |
| `MANDATE-PAYMENT-001` | Per-payment cap at request |
| `MANDATE-PAYMENT-002` | Per-payment cap at execute |
| `MANDATE-DAILY-001` | Daily usage per recipient |
| `MANDATE-DAILY-002` | Daily cap at request |
| `MANDATE-DAILY-003` | Daily cap at execute |
| `MANDATE-APPROVAL-001` | Which payouts need approval |
| `MANDATE-APPROVAL-002` | Approvals required |
| `MANDATE-APPROVAL-003` | Repeated approval |
| `MANDATE-APPROVAL-004` | M of N approval |
| `MANDATE-APPROVAL-005` | A single rejection is a veto |
| `MANDATE-APPROVAL-006` | Approver removal keeps the gate working |
| `MANDATE-APPROVAL-007` | Adding the first approver |
| `MANDATE-APPROVAL-008` | Removing the last approver |
| `MANDATE-APPROVAL-009` | Threshold re-checked when there are no approvers |
| `MANDATE-BATCH-001` | Request batch length |
| `MANDATE-BATCH-002` | Request batch runs per item |
| `MANDATE-BATCH-003` | Cancel batch |
| `MANDATE-BATCH-004` | Execute batch length |
| `MANDATE-BATCH-005` | Execute batch is atomic |
| `MANDATE-BATCH-006` | Transfer budget per execute batch |
| `MANDATE-BATCH-007` | Allowlist batch length |
| `MANDATE-PAUSE-001` | Paused mandates refuse new work |
| `MANDATE-PAUSE-002` | Pause |
| `MANDATE-PAUSE-003` | Resume |
| `MANDATE-REVOKE-001` | Revoke is a pure state change |
| `MANDATE-REVOKE-002` | Revoked mandates refuse state changes |
| `MANDATE-REVOKE-003` | Return path after revoke |
| `MANDATE-WITHDRAW-001` | Withdraw goes to the principal only |
| `MANDATE-WITHDRAW-002` | Withdraw amount |
| `MANDATE-WITHDRAW-003` | Withdraw is capped at the free balance |
| `MANDATE-WITHDRAW-004` | Withdraw effects |
| `MANDATE-SWEEP-001` | Sweep needs a closed mandate |
| `MANDATE-SWEEP-002` | No third-party sweep during a principal change |
| `MANDATE-SWEEP-003` | Grace period after expiry |
| `MANDATE-SWEEP-004` | Nothing to sweep |
| `MANDATE-SWEEP-005` | Sweep effects |
| `MANDATE-RECOVER-001` | Token recovery is for other tokens |
| `MANDATE-RECOVER-002` | Token recovery amount |
| `MANDATE-RECOVER-003` | Token recovery effects |
| `MANDATE-RECOVER-004` | Token recovery never moves the settlement asset |
| `MANDATE-PRINCIPAL-001` | Propose a new principal |
| `MANDATE-PRINCIPAL-002` | Accept the transfer |
| `MANDATE-PRINCIPAL-003` | Cancel a proposal |
| `MANDATE-PRINCIPAL-004` | The principal changes only in two steps |
| `MANDATE-PRINCIPAL-005` | Accept needs a proposal |
| `MANDATE-PRINCIPAL-006` | Roles are re-checked at acceptance |
| `MANDATE-FEE-001` | One optional fee on top of the total |
| `MANDATE-FEE-002` | Fee cap |
| `MANDATE-FEE-003` | Fee within the cap |
| `MANDATE-FEE-004` | Which rules use the charge |
| `MANDATE-FEE-005` | Fee rate set at creation |
| `MANDATE-FEE-006` | The fee cap can only go down |
| `MANDATE-FEE-007` | The fee address is fixed |
| `MANDATE-FEE-008` | Only execute pays the fee |
| `MANDATE-EVENT-001` | Exactly the specified events |
| `MANDATE-EVENT-002` | Canonical form |
| `MANDATE-EVENT-003` | Funding has no mandate event |
| `CHAINOPS-XCHAIN-001` | Fail over only after a provable death |
| `CHAINOPS-DELIVERY-001` | Forwarders are bound by recomputation |
| `CHAINOPS-DELIVERY-002` | Partner deposit addresses need partner confirmation |
| `CHAINOPS-DELIVERY-003` | Only payable Stellar destinations are bindable |
| `CHAINOPS-DELIVERY-004` | Providers are certified before use |
| `CHAINOPS-SUBMIT-001` | No memo on contract calls |
| `CHAINOPS-SUBMIT-002` | Submission discipline on Stellar |
| `CHAINOPS-SUBMIT-003` | Simulate attested requests before submitting |
| `CHAINOPS-SUBMIT-004` | Cancel lapsed payouts while their records are live |
| `CHAINOPS-CREATE-001` | The pause-only guardian is on by default |
| `CHAINOPS-CREATE-002` | Large mandates need more than one key |
| `CHAINOPS-CREATE-003` | Custody guard for operator-held agent keys |
| `CHAINOPS-CREATE-004` | The attester key signs for no role account |
| `CHAINOPS-CREATE-005` | Attester key form |
| `CHAINOPS-SWEEP-001` | Every sweep is booked once |
| `CHAINOPS-SWEEP-002` | Sweep notifications are grouped |
| `CHAINOPS-SWEEP-003` | Third-party funding of closed mandates alerts |
| `CHAINOPS-SWEEP-004` | The operator's sweeper respects the guards |
| `CHAINOPS-FEE-001` | The fee is shown and reconciled |
| `CHAINOPS-FEE-002` | The request builder sets the fee |
| `CHAINOPS-FEE-003` | The fee address is a dedicated multisig |
| `CHAINOPS-FEE-004` | The fee on a refunded part is returned |
| `CHAINOPS-FEE-005` | A fee address that cannot receive |
