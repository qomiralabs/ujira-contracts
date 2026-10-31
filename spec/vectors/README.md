# Shared test vectors

A vector states, as data, what the mandate must do in one scenario: a starting state, a sequence of steps, and the exact result of every call. The contract runs the vectors unchanged, and a vector that fails is a failing build. When a vector and `spec/MANDATE.md` disagree, the vector is wrong.

The format is chain-neutral. UJIRA runs on Stellar only and only the Stellar runner is built, but a case that only another chain could express is kept, restricted to that chain, so an implementation added later inherits it.

`scripts/check-vectors.sh` checks every file here. It is described at the end, after the one complete example.

## Files

- `schema.json`: the JSON Schema (draft 2020-12) of the format.
- `example.json`: the complete example at the end of this file.
- Every other `*.json` file here is a vector file: `{ "format": "ujira-mandate-vectors/1", "description": "...", "vectors": [ ... ] }`, where `description` is optional.

## Values

Every value has exactly one written form, so a runner never rounds, guesses or converts.

| Value | Written as | Example |
| --- | --- | --- |
| Amount | A decimal string of raw minor units of the mandate's asset: at most 39 digits, no leading zero, never a JSON number. The contract never reads decimals, so one vector runs on any token scale. | `"1000"` |
| u64: times, tags, epochs and durations | A decimal string from `"0"` to `"18446744073709551615"`, no leading zero. Times are unix seconds. | `"1767225600"` |
| Small integer: counts, codes and basis points | A JSON integer from 0 to 4294967295. | `0` |
| Address | An actor name: lower case letters, digits and `_`, starting with a letter. | `"agent"` |
| 32 bytes | `0x` and 64 lower case hex characters, or a label `#name`. | `"#p1"` |
| Error | A name from `spec/errors.json`. | `"MANDATE_PAUSED"` |
| Status | `PENDING`, `APPROVED`, `EXECUTED`, `REJECTED` or `CANCELLED`. | `"APPROVED"` |
| Recipient mode | `ALLOWLIST`, `ATTESTED` or `BOTH`. | `"ALLOWLIST"` |

Role codes, recovery action kinds and cancel reasons are the small integers of `spec/constants.json` and `spec/MANDATE.md`. No number anywhere has a fraction or an exponent.

A value is written as given even when the contract refuses it, so a vector can test the refusal: an amount of `"0"`, a role code of `9` or a `max_fee_bps` of `101`. A value only one chain's types can hold restricts the vector to that chain (see Chains): a negative amount fits only a Stellar `i128`, and an amount above `I128_MAX` fits only a Celo `uint256` argument.

### Actors

Each actor name stands for one address, the same for the whole vector, which the runner creates. An actor is an account unless `setup.actors` lists it as a contract. These names are reserved:

| Name | What it is |
| --- | --- |
| `mandate` | The mandate, created by the setup or by the one `create` call that succeeds. |
| `asset` | The settlement asset, which the factory supports. |
| `other_token` | A second token, which the factory does not support. |
| `fee_address` | The factory's fee address, an account unless the setup lists it as a contract, as a vector does when it pays a fee above what a Stellar trustline holds. It may make calls, so a vector can show that it holds no role. |
| `zero` | The zero address. Only Celo has one, so a vector that names it is restricted to Celo. |

`mandate`, `asset`, `other_token` and `zero` never make a call.

### Labels and test keys

A label `#name` stands for 32 fixed bytes: the SHA-256 of the ASCII string `UJIRA/TEST-ONLY/vector/` followed by the name. Two labels give the same bytes exactly when their names are equal. Where a key is expected, in an attester and in an attest step's `signer`, the label names the ed25519 test key whose seed is those 32 bytes, and stands for its public key. These keys are public by construction: they never hold funds, sign anything real or serve as an attester outside tests.

An attester is written as one of:

- `null`: no attester, in a configuration only;
- a label: the public key of that test key;
- `0x` and 0 to 64 bytes of lower case hex: raw bytes, so a key of 31 or 33 bytes, or of 32 zero bytes, can be expressed;
- `{ "account_key": "<actor>" }`: the ed25519 key behind an account actor's address, to test that the attester key holds no role.

### Destinations

A destination is `{ "address": "<actor>", "tag": null }` or `{ "address": "<actor>", "tag": "<u64>" }`, wherever it appears: legs, allowlist entries, attest steps, events and state. Only an account takes a tag. On Stellar the runner passes the muxed `M` address of that account and tag; on Celo the tag travels as `hasTag` and `tag` beside the address.

Celo can also express a tag marked absent that still carries a value, which Stellar cannot. It is written `"tag": { "has_tag": false, "tag": "7" }`, with a non-zero tag, and only in a vector restricted to Celo.

## Vectors

| Field | Meaning |
| --- | --- |
| `id` | Unique across every file: lower case letters, digits and `_`. |
| `rules` | The rule IDs the vector tests, from `spec/MANDATE.md`. A retired rule is never cited. |
| `description` | One line of plain ASCII. |
| `applies_to`, `skip_reason` | Optional. See Chains. |
| `setup` | The starting state. |
| `steps` | What happens, in order. |

### Chains

A vector runs on every chain, `stellar` and `celo`, unless `applies_to` lists fewer. A restricted vector needs `"skip_reason": "unrepresentable_input"`, the only reason there is: the chains it leaves out cannot express its input. Listing every chain, or giving a skip reason without a restriction, is refused. These inputs need a restriction:

| Input | Runs only on | Why |
| --- | --- | --- |
| A negative amount | `stellar` | Celo amounts are unsigned. |
| An amount above `I128_MAX` | `celo` | Stellar amounts are `i128`. |
| The `zero` actor | `celo` | Stellar has no zero address. |
| The non-canonical tag | `celo` | A Stellar destination cannot mark a tag absent and still carry one. |
| A `reentrant` test token | `celo` | The Stellar host refuses any call into a contract already on the call stack, views included. |
| `recover_token` of a `moves_settlement` token | `celo`, or `stellar` expecting `TRANSFER_FAILED` | Only on Celo can a second token address move the settlement balance; on Stellar the token's attempt fails its transfer. |
| A `returns_value` test token | `stellar` | Only a Stellar transfer returns nothing, so only there is a returned value unexpected. |
| A `returns_no_data` test token | `celo` | Only an EVM call can return no data at all. |
| A `set_receiving` step to `no_trustline` | `stellar` | Only Stellar has trustlines. |
| An attester written `{ "account_key": "<actor>" }` | `stellar` | Only a Stellar account address is an ed25519 key. |

## Setup

| Field | Meaning |
| --- | --- |
| `now` | Required. The ledger time, a u64. |
| `actors` | `{ "<name>": { "kind": "contract" } }` for each actor that is a contract. |
| `tokens` | Test tokens for `asset` and `other_token`; each is `standard` unless listed. |
| `mandate` | The mandate's configuration. Without it, the vector creates the mandate with a `create` call. |
| `salt` | The salt the setup creates the mandate with. Defaults to `"#salt"`. |
| `allowlist` | Entries `{ "recipient_ref", "destination" }`. |
| `mints` | `{ "token", "to", "amount" }`, minted by the test token's admin. |
| `transfers` | `{ "token", "from", "to", "amount" }`, plain token transfers. |

The runner applies the setup in this order, none of it may fail, and none of its events are expected anywhere: it sets the time to `now`, creates the mandate through the factory as its principal with `salt`, allows the allowlist entries as the principal (an `ATTESTED` mandate has no allowlist), then makes the mints and the transfers in order.

A configuration has every one of these fields: `principal`, `asset`, `recipient_mode`, `tenant_ref`, `agents`, `approvers`, `approvals_required`, `guardians`, `executors`, `recovery`, `recovery_threshold`, `limits`, `attester`, `country_policy_hash` and `max_fee_bps`. `limits` has `total_budget`, `per_payment_cap`, `per_recipient_daily_cap`, `approval_threshold`, `payout_ttl_seconds` and `expires_at`. The fee address comes from the factory and is never part of it.

Test tokens:

| Kind | Behaviour |
| --- | --- |
| `standard` | A plain token. |
| `reentrant` | Celo only. Its transfer calls `reenters` on the mandate, with the token as the caller: `sweep`, or `cancel` or `execute` of `payout_id`. With `"variant": "propagate"` the refused nested call fails the token's transfer; with `"variant": "catch"` the token catches the refusal and completes the transfer. |
| `moves_settlement` | `other_token` only. Its transfer also tries to move the mandate's settlement balance, as a second address for the same asset would. |
| `returns_value` | `other_token` only, Stellar only. Its transfer moves the tokens and then returns a value instead of nothing. |
| `returns_no_data` | `other_token` only, Celo only. Its transfer moves the tokens and returns no data, as some older ERC-20 tokens do. |

Every account starts able to receive both tokens: on Stellar the runner gives it an authorized trustline for each. A `set_receiving` step changes that for the settlement asset.

## Steps

Each step names its kind in `"step"`:

| Step | Fields | What it does |
| --- | --- | --- |
| `call` | `fn`, `caller`, `signed_by`, `args`, `expect` | Calls the factory or the mandate at the current time. |
| `advance_time` | `seconds` | Moves the ledger time forward. |
| `set_time` | `now` | Sets the ledger time, never to before the current time. |
| `transfer` | `token`, `from`, `to`, `amount`, `events` | A plain token transfer that does not call the mandate, such as funding it. |
| `mint` | `token`, `to`, `amount` | Mints the test token. |
| `attest` | `name`, `signer`, `recipient_ref`, `destination`, `valid_until`, `policy_epoch`, `tamper` | Makes an attestation for a later request. |
| `set_receiving` | `token`, `account`, `state` | Sets whether the account actor `account` can receive `token`, which is always `asset`. |
| `clawback` | `token`, `from`, `amount` | The issuer of `token`, always `asset`, takes `amount` from `from` without its authorization, which is how an asset can remove the mandate's balance. On Stellar the runner issues the asset with clawback enabled; on Celo the test token burns it. It never calls the mandate. |

Only a call has an expectation; the other steps cannot fail. A transfer may carry `"events": []`, the only value it takes, and the runner then checks that the mandate and the factory emit nothing during it, which is how a vector shows that funding is not a mandate call.

The states of `set_receiving`:

| State | Stellar | Celo |
| --- | --- | --- |
| `receivable` | The account holds an authorized trustline, opened if it had none. | The test token stops blocking transfers to the account. |
| `deauthorized` | The issuer clears the authorization of the account's trustline, opened first if it had none. | The test token refuses every transfer to the account. |
| `no_trustline` | The account's trustline is removed. Stellar removes a trustline only at a zero balance, so the account holds none of the asset at that point. | Not expressible. |

### Calls

`caller` makes the call. For a function that takes a caller argument, `caller` is also that argument, and `signed_by`, when given, is the actor whose authorization the call carries instead; that is how a vector names the principal as the caller without the principal's signature. Without `signed_by` a call is signed by its caller. A function without a caller argument authorizes a fixed address, so `caller` is simply who signs, and `signed_by` is refused there.

| Function | Arguments | Return |
| --- | --- | --- |
| `create` | `config`, `salt` | `"mandate"` |
| `request_payout` | `req` | A status |
| `request_payouts` | `reqs` | Item results |
| `approve` | `payout_id` | A status |
| `reject`, `cancel`, `execute` | `payout_id` | None |
| `cancel_batch` | `ids` | Item results |
| `execute_batch` | `ids` | None |
| `pause`, `resume`, `accept_principal`, `cancel_principal_proposal` | None | None |
| `revoke`, `sweep` | None | An amount |
| `recovery_revoke` | None | `1` when a vote was recorded, `2` when the action ran |
| `recovery_rotate_agent` | `old`, and `new` (an actor or null) | `1` or `2` |
| `withdraw` | `amount` | None |
| `recover_token` | `token`, `amount` | None |
| `update_limits` | `limits` | None |
| `add_role`, `remove_role` | `role`, `account` | None |
| `set_approvals_required`, `set_recovery_threshold` | `n` | None |
| `allow_recipients`, `disallow_recipients` | `entries` | None |
| `set_attestation_policy` | `attester` (never null), `country_policy_hash` | None |
| `propose_principal` | `proposed` | None |
| `lower_fee_cap` | `new_bps` | None |
| `version` | None | A small integer |
| `config` | None | An object |
| `state` | None | `{ spent, pending, committed, balance, free, paused, revoked, expired, policy_epoch }` |
| `payout` | `payout_id` | The record, the tombstone `{ payout_id, status }`, or null |
| `daily_used` | `recipient_ref` | `{ day, used }` |
| `is_allowed` | `recipient_ref`, `destination` | A boolean |

A request, `req` or each entry of `reqs`, is `{ payout_id, recipient_ref, total, fee, legs, deadline, task_ref, attestations }`. `fee` defaults to `"0"`, the only default in a request. `legs` is a list of `{ destination, amount }`, and `attestations` names attestations made by earlier attest steps, in leg order. A batch item result is `{ payout_id, code, status }`, where `code` is `0` for success or the item's error name, and `status` is null when the item has none.

`create` makes the vector's mandate when the setup has none, and a vector has one mandate; every other function needs it. On Stellar a constructor failure reaches the caller of `create` only as the generic host error `Error(Context, InvalidAction)`. Seeing it means the factory skipped a check it must make before deploying, so the format has no way to expect it.

### Precedence

A refused call may carry `"precedence": ["3", "4"]`: it fails checks 3 and 4 of its function's check order in `spec/MANDATE.md` at once, so the earlier check must decide the outcome. Two contracts can each pass every vector that breaks one rule and still disagree when several break together, and these calls pin which one wins. The two labels are a check and the one right after it, the vector cites both checks' rules, and the call is refused the way the earlier check refuses:

- with one of the errors on the earlier rule's Error line;
- with `"per_chain": { "stellar": { "host_auth_failure": true } }` when that rule fails host authorization on Stellar, such as `MANDATE-ROLE-019` or `MANDATE-ROLE-015`, and with `host_abort` when it is the signature check, `MANDATE-RECIPIENT-012`; on the other chain, where `MANDATE-ROLE-019` cannot fail, the signer is the caller and the next check decides;
- with no per-chain override when the earlier rule is a mandate error on every chain;
- for a check that hands off to per-item checks, such as `execute_batch` check 7, with the item's own error, never the later check's.

`spec/vectors/precedence.json` holds a call for most pairs. A pair that an earlier vector already pinned, such as the fee cap before the per-payment cap, carries its `precedence` where it is. Where both checks return the same error, as most role conflicts at `create` do, the call shows that error and not which check produced it; the checker counts those pairs.

### Attestations

The test key `signer` signs an attestation for one leg: the version 2 message of `spec/ENCODING.md` over the network the mandate runs on, the mandate's address, `recipient_ref`, the mandate's country policy hash, `policy_epoch` (by default the mandate's current one), `valid_until` and `destination`. The attestation carries `valid_until`, `policy_epoch` and the signature, under its `name`. The runner's attest step must produce, byte for byte, the fields, messages and signatures of the reference signer in `spikes/attestation-parity/generator/` for every case of `spikes/attestation-parity/fixture.json`.

`tamper` damages the attestation in one exact way. The first ten kinds leave a 64-byte signature that fails ed25519 verification, which the Stellar host turns into a trap the contract cannot catch. The last five leave a signature that is not 64 bytes, which every chain refuses with `ATTESTATION_INVALID` before verifying.

| Tamper | What changes |
| --- | --- |
| `wrong_signer` | The test key `#wrong_signer` signs instead of `signer`. |
| `wrong_chain` | The signed network ID has the lowest bit of its last byte flipped. |
| `wrong_mandate` | The signed mandate address has the lowest bit of its last byte flipped. |
| `wrong_destination` | The signed destination address has the lowest bit of its last byte flipped. |
| `wrong_tag` | The signed tag differs: no tag is signed as tag `0`, and tag `t` as `t` XOR 1. |
| `wrong_policy` | The signed country policy hash has the lowest bit of its last byte flipped. |
| `wrong_ref` | The signed `recipient_ref` has the lowest bit of its last byte flipped. |
| `wrong_valid_until` | The signed `valid_until` is the carried one XOR 1. |
| `flipped_bit` | The lowest bit of the signature's first byte is flipped. |
| `non_canonical_s` | S, the last 32 bytes, becomes S + L in 32 bytes little endian, where L is the order of the ed25519 group. |
| `empty` | The signature has no bytes. |
| `truncated` | The signature is cut to its first 32 bytes. |
| `short_signature` | The signature is cut to its first 63 bytes. |
| `long_signature` | One zero byte is appended, for 65 bytes. |
| `extended` | 64 zero bytes are appended, for 128 bytes. |

The tamper kinds of the secp256k1 attestations, which were never deployed (`high_s`, `bad_v`, `r_zero`, `s_zero`, `r_order`, `r_not_on_curve` and `recovers_to_infinity`), are retired and refused.

An untampered attestation also fails verification when a request uses it for a leg whose destination differs from the attestation's, for another `recipient_ref`, or after the attester changed, because the contract builds the message from the leg, the request and the attester it holds at that moment. A vector writes that as it happens: an attestation made for one tag and used for a leg with another, or two attestations given in the wrong leg order.

## Expectations

Every call has one `expect`:

- `{ "ok": true, "return": ..., "events": [ ... ], "token_events": [], "state": { ... } }` for success, where `return`, `token_events` and `state` are optional;
- `{ "error": "<NAME>", "events": [] }` for a refusal: the first failing check of the operation's check order, with nothing written and nothing emitted.

`events` lists every event the mandate and the factory emit during the call, in order, and nothing else, so a missing or an extra event fails. Token events are not listed. `token_events` takes only `[]`, and says that no token emits any event during the call, so the call moved no tokens; without it token events are not compared. Each event is `{ "name": "...", "fields": { ... } }` with exactly the fields `spec/EVENTS.md` gives it, in canonical form: the topic key is a field like any other, destinations are `{ address, tag }`, and an error code is its name.

`return` is compared exactly. `state` is compared after the call, and only on what it names:

- `mandate`: any of `spent`, `pending`, `committed`, `balance`, `free`, `paused`, `revoked`, `expired`, `policy_epoch`, `recovery_epoch`, `agent_epoch`, `approvals_required`, `recovery_threshold`, `max_fee_bps`, `principal`, `proposed_principal`, the five role sets, `limits`, `attester` and `country_policy_hash`;
- `payouts`: by payout ID, any fields of the record, or `status` alone for a tombstone, or null when there is neither;
- `balances`: by token, then by actor;
- `daily`: by `recipient_ref`, `{ day, used }`.

### Per-chain expectations

`expect` may carry `"per_chain": { "stellar": { ... } }`, saying what happens on Stellar instead while every other chain keeps the expectation around it. There are two overrides, and each means the call fails with no state change and no events:

- `{ "host_abort": "Crypto.InvalidInput" }`: ed25519 verification traps in the host. Allowed only on `request_payout` and `request_payouts` calls carrying an attestation with one of the ten invalid-signature tamper kinds, or an untampered one made for another destination or `recipient_ref` than the leg and request it is used with, or signed by a test key that is not the attester when the request is made, and the expectation around it must be `{ "error": "ATTESTATION_INVALID", "events": [] }`. A `request_payouts` call that traps fails as a whole, so it never expects `payout_request_rejected` events for its other items.
- `{ "host_auth_failure": true }`: the call fails host authorization. Allowed on a call without a caller argument, whose signer is not the address it must authorize, and on a call with a caller argument that `signed_by` signs instead, in a vector that cites `MANDATE-ROLE-019`. Never on a view, which needs no authorization.

## Checking

`scripts/check-vectors.sh` validates every vector file against `schema.json` with check-jsonschema, which first checks `schema.json` against the JSON Schema metaschema, and then runs `scripts/check-vectors.py`, which checks what a schema cannot:

- every cited rule is in `spec/MANDATE.md` and not retired, every error is in `spec/errors.json`, and every event is in `spec/EVENTS.md` with exactly its fields;
- vector IDs are unique across files, no number has a fraction, and no object repeats a key;
- every input that only one chain can express is in a vector restricted to that chain, and only accounts take tags or change whether they can receive;
- requests name only attestations made earlier, the secp256k1 tamper kinds are refused, and the per-chain overrides appear only where the rules above allow them;
- calls come after the mandate exists, and time never moves back;
- every `precedence` call names two adjacent checks of its function's check order and is refused as the Precedence section says.

Coverage is checked on request. `--require-all` needs every `MANDATE` rule that is not retired to have at least one passing and one failing vector, and `--areas A,B` needs the same of the rules in the named areas only, so each vector set can prove its own coverage. A vector is failing when any call in it is refused on some chain: an error, a per-chain override, a `payout_request_rejected` event or a batch item with a non-zero code. Every other vector is passing. `CHAINOPS` rules bind the off-chain services and never count as uncovered.

Parity with the attestation fixture is checked with `--fixture spikes/attestation-parity/fixture.json`, as CI does: every case of the fixture needs a vector named `parity_` and the case name that runs the same case through the mandate, and every `parity_` vector must name a case. Six cases are exempt because an attest step cannot build them, and the check prints them: `valid_policy_epoch_u64_max` (no call sequence raises the policy epoch that far), `wrong_policy_epoch_signed` (an attest step signs the epoch it carries), and `flipped_s_bit`, `zero_signature`, `identity_r_zero_s` and `small_order_attester_key` (no tamper kind writes those signature bytes). The contract passes every 64-byte signature to the host unchanged, so those four test the host's verifier, and the attest step itself is still compared with every case of the fixture.

Precedence is checked with `--precedence`: every adjacent pair of checks in every check order of `spec/MANDATE.md` needs a `precedence` call in the files checked. Eleven pairs are exempt because no input can make both checks fail, and the check prints them; each exemption names the two rules it was written for, so a renumbered or rewritten order fails until the exemption is updated:

- `request_payout` 16 and 17: check 16 applies only in `ALLOWLIST` mode and check 17 only in `ATTESTED` and `BOTH`;
- `request_payout` 19 and 20: an expired attestation has `valid_until` at or before `now`, so its window is never too long;
- `add_role` 4 and 5, and `propose_principal` 2 and 3: the zero address is never the principal, the fee address or a role member;
- `remove_role` 3 and 4, 4 and 5, and 5 and 6: check 4 applies only to an approver in the set, to one among several, where check 5 applies to the only one, and check 6 to role 5;
- `add_role` 5 and 6, `propose_principal` 3 and 4, `accept_principal` 3 and 4, and `recovery_rotate_agent` 5 and 6: an account that conflicts is the principal, the fee address or a role member, and the attester's key never matches any of those (`MANDATE-ROLE-018` holds on every path that sets either).

check-jsonschema is pinned, with a hash for every dependency, in `scripts/check-vectors.requirements.txt`, and the check refuses any other version. To run the check and its self test locally:

```sh
python3 -m venv ~/.venvs/check-jsonschema
~/.venvs/check-jsonschema/bin/pip install --require-hashes --no-deps --only-binary :all: -r scripts/check-vectors.requirements.txt
export CHECK_JSONSCHEMA=~/.venvs/check-jsonschema/bin/check-jsonschema
scripts/check-vectors.sh
scripts/check-vectors.test.sh
```

The `vectors` job in CI runs both, with the same pinned version, and runs the check as `scripts/check-vectors.sh --require-all --precedence --fixture spikes/attestation-parity/fixture.json`, so a new rule without its vectors, or a new check without its precedence calls, fails the build.

## Complete example

A guardian pauses the mandate. While it is paused, a request is refused with `MANDATE_PAUSED` and a second pause with `ALREADY_PAUSED`, and neither writes or emits anything. The principal resumes, and the same request, with the same payout ID because the refused one wrote nothing, is approved at once: the mandate has no approvers, so its approval threshold is above the per-payment cap and every payout within the cap is `APPROVED` on request. The payout expires one time to live, 86400 seconds, after it was requested. This is `example.json`:

```json
{
  "format": "ujira-mandate-vectors/1",
  "description": "The example vector from spec/vectors/README.md, kept as a file so the checker and the runner see exactly what the README shows.",
  "vectors": [
    {
      "id": "pause_blocks_requests_until_resume",
      "rules": ["MANDATE-PAUSE-001", "MANDATE-PAUSE-002", "MANDATE-PAUSE-003"],
      "description": "A guardian pauses, a request and a second pause are refused, and once the principal resumes the same request is approved.",
      "setup": {
        "now": "1767225600",
        "mandate": {
          "principal": "principal",
          "asset": "asset",
          "recipient_mode": "ALLOWLIST",
          "tenant_ref": "#tenant",
          "agents": ["agent"],
          "approvers": [],
          "approvals_required": 0,
          "guardians": ["guardian"],
          "executors": [],
          "recovery": [],
          "recovery_threshold": 0,
          "limits": {
            "total_budget": "10000",
            "per_payment_cap": "1000",
            "per_recipient_daily_cap": "500",
            "approval_threshold": "1001",
            "payout_ttl_seconds": "86400",
            "expires_at": "1798761600"
          },
          "attester": null,
          "country_policy_hash": "#no_countries",
          "max_fee_bps": 0
        },
        "allowlist": [
          { "recipient_ref": "#bob", "destination": { "address": "bob", "tag": null } }
        ],
        "mints": [{ "token": "asset", "to": "principal", "amount": "1000" }],
        "transfers": [{ "token": "asset", "from": "principal", "to": "mandate", "amount": "1000" }]
      },
      "steps": [
        {
          "step": "call",
          "fn": "pause",
          "caller": "guardian",
          "expect": {
            "ok": true,
            "events": [{ "name": "paused", "fields": { "by": "guardian" } }],
            "state": { "mandate": { "paused": true } }
          }
        },
        { "step": "advance_time", "seconds": "60" },
        {
          "step": "call",
          "fn": "request_payout",
          "caller": "agent",
          "args": {
            "req": {
              "payout_id": "#p1",
              "recipient_ref": "#bob",
              "total": "100",
              "fee": "0",
              "legs": [{ "destination": { "address": "bob", "tag": null }, "amount": "100" }],
              "deadline": "1767226200",
              "task_ref": "#task1",
              "attestations": []
            }
          },
          "expect": { "error": "MANDATE_PAUSED", "events": [] }
        },
        {
          "step": "call",
          "fn": "pause",
          "caller": "guardian",
          "expect": { "error": "ALREADY_PAUSED", "events": [] }
        },
        {
          "step": "call",
          "fn": "resume",
          "caller": "principal",
          "expect": {
            "ok": true,
            "events": [{ "name": "resumed", "fields": { "by": "principal" } }],
            "state": { "mandate": { "paused": false } }
          }
        },
        {
          "step": "call",
          "fn": "request_payout",
          "caller": "agent",
          "args": {
            "req": {
              "payout_id": "#p1",
              "recipient_ref": "#bob",
              "total": "100",
              "fee": "0",
              "legs": [{ "destination": { "address": "bob", "tag": null }, "amount": "100" }],
              "deadline": "1767226200",
              "task_ref": "#task1",
              "attestations": []
            }
          },
          "expect": {
            "ok": true,
            "return": "APPROVED",
            "events": [
              {
                "name": "payout_requested",
                "fields": {
                  "payout_id": "#p1",
                  "agent": "agent",
                  "recipient_ref": "#bob",
                  "total": "100",
                  "fee": "0",
                  "legs": [{ "destination": { "address": "bob", "tag": null }, "amount": "100" }],
                  "task_ref": "#task1",
                  "status": "APPROVED",
                  "expires_at": "1767312060"
                }
              }
            ],
            "state": {
              "mandate": { "spent": "0", "pending": "0", "committed": "100", "balance": "1000", "free": "900" },
              "payouts": {
                "#p1": { "status": "APPROVED", "total": "100", "fee": "0", "created_at": "1767225660", "expires_at": "1767312060" }
              },
              "balances": { "asset": { "mandate": "1000", "principal": "0" } }
            }
          }
        }
      ]
    }
  ]
}
```
