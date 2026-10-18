# Events

The canonical event set of spec version 1, in force on Stellar. Event shapes are frozen per spec version: a new field means a new event name in a new spec version (`MANDATE-EVENT-002`). `scripts/check-spec-ids.sh` checks that this table and the event list in `MANDATE.md` name the same events with the same fields.

## Encoding on Stellar

- Topics: `[Symbol(name), key]`, where `key` is the `payout_id` (`BytesN<32>`) for payout events and the mandate's `tenant_ref` (`BytesN<32>`) for every other event, including `mandate_created` from the factory.
- Data: one `#[contracttype]` struct per event whose fields are exactly the fields below, in that order, with snake_case names.
- Destinations: every destination, including each entry of `legs`, is the canonical pair `{ address, tag }` with `tag` null when absent (`MANDATE-EVENT-002`), encoded as a `MuxedAddress`, so an `M` address carries the tag.
- Amounts are `i128`, times are `u64` unix seconds, `day` is `u64` (`now / SECONDS_PER_DAY`), `policy_epoch` is `u64` (`MANDATE-RECIPIENT-017`), other counts and epochs are `u32`, role and kind codes are `u32` with the values in `constants.json`.
- Funding emits no mandate event; the asset's own transfer event is the record (`MANDATE-EVENT-003`).

## Events

| Event | Fields | Topic key |
| --- | --- | --- |
| `mandate_created` (factory) | mandate, principal, asset, tenant_ref, spec_version, fee_address | tenant_ref |
| `mandate_initialized` | principal, asset, recipient_mode, tenant_ref, agents, approvers, approvals_required, guardians, executors, recovery, recovery_threshold, limits, attester, country_policy_hash, max_fee_bps, fee_address | tenant_ref |
| `payout_requested` | payout_id, agent, recipient_ref, total, fee, legs, task_ref, status, expires_at | payout_id |
| `payout_request_rejected` | payout_id, agent, code | payout_id |
| `payout_approval_added` | payout_id, approver, approvals, required | payout_id |
| `payout_approved` | payout_id | payout_id |
| `payout_rejected` | payout_id, by | payout_id |
| `payout_cancelled` | payout_id, by, reason | payout_id |
| `payout_executed` | payout_id, recipient_ref, total, fee, legs, day, spent | payout_id |
| `fee_cap_lowered` | max_fee_bps | tenant_ref |
| `paused`, `resumed` | by | tenant_ref |
| `revoked` | by, frozen | tenant_ref |
| `withdrawn` | to, amount | tenant_ref |
| `swept` | by, amount | tenant_ref |
| `token_recovered` | token, amount | tenant_ref |
| `limits_updated` | total_budget, per_payment_cap, per_recipient_daily_cap, approval_threshold, payout_ttl_seconds, expires_at | tenant_ref |
| `role_added`, `role_removed` | role, account | tenant_ref |
| `approvals_required_set` | required | tenant_ref |
| `recovery_threshold_set` | required, recovery_epoch | tenant_ref |
| `recovery_vote_added` | kind, old, new, votes, required, recovery_epoch | tenant_ref |
| `recovery_executed` | kind, old, new, by | tenant_ref |
| `recipient_allowed`, `recipient_disallowed` | recipient_ref, destination | tenant_ref |
| `attestation_policy_set` | attester, country_policy_hash, policy_epoch | tenant_ref |
| `principal_proposed` | current, proposed | tenant_ref |
| `principal_transferred` | previous, current | tenant_ref |
| `principal_proposal_cancelled` | current, proposed | tenant_ref |
