# Events

The canonical event set of spec version 1, in force on Stellar. Event shapes are frozen per spec version: a new field means a new event name in a new spec version (`MANDATE-EVENT-002`). `scripts/check-spec-ids.sh` checks that this table and the event list in `MANDATE.md` name the same events with the same fields.

## Encoding on Stellar

- Topics: `[Symbol(name), key]`, where `key` is the `payout_id` (`BytesN<32>`) for payout events and the mandate's `tenant_ref` (`BytesN<32>`) for every other event, including `mandate_created` from the factory.
- Data: a map of the event's fields with snake_case names, keyed by name, excluding the field used as the topic key. The topic key is carried only in the topics, and decoders restore it into the canonical fields (`MANDATE-EVENT-002`). For an event whose key is `tenant_ref` and whose field list does not include it, `tenant_ref` is a topic only. Repeating the key in the data would add about 60 bytes to every payout event and break the events headroom behind `MAX_BATCH_TRANSFERS`. An event whose only field is its topic key, such as `payout_approved`, still has data: an empty map, never an absent value, and the table writes it as `an empty map`.
- Destinations: every destination, including each entry of `legs`, is the canonical pair `{ address, tag }` with `tag` null when absent (`MANDATE-EVENT-002`), encoded as a `MuxedAddress`, so an `M` address carries the tag.
- Amounts are `i128`, times are `u64` unix seconds, `day` is `u64` (`now / SECONDS_PER_DAY`), `policy_epoch` is `u64` (`MANDATE-RECIPIENT-017`), other counts and epochs are `u32`, role and kind codes are `u32` with the values in `constants.json`.
- Funding emits no mandate event; the asset's own transfer event is the record (`MANDATE-EVENT-003`).

## Events

| Event | Fields | Topic key | Data |
| --- | --- | --- | --- |
| `mandate_created` (factory) | mandate, principal, asset, tenant_ref, spec_version, fee_address | tenant_ref | mandate, principal, asset, spec_version, fee_address |
| `mandate_initialized` | principal, asset, recipient_mode, tenant_ref, agents, approvers, approvals_required, guardians, executors, recovery, recovery_threshold, limits, attester, country_policy_hash, max_fee_bps, fee_address | tenant_ref | principal, asset, recipient_mode, agents, approvers, approvals_required, guardians, executors, recovery, recovery_threshold, limits, attester, country_policy_hash, max_fee_bps, fee_address |
| `payout_requested` | payout_id, agent, recipient_ref, total, fee, legs, task_ref, status, expires_at | payout_id | agent, recipient_ref, total, fee, legs, task_ref, status, expires_at |
| `payout_request_rejected` | payout_id, agent, code | payout_id | agent, code |
| `payout_approval_added` | payout_id, approver, approvals, required | payout_id | approver, approvals, required |
| `payout_approved` | payout_id | payout_id | an empty map |
| `payout_rejected` | payout_id, by | payout_id | by |
| `payout_cancelled` | payout_id, by, reason | payout_id | by, reason |
| `payout_executed` | payout_id, recipient_ref, total, fee, legs, day, spent | payout_id | recipient_ref, total, fee, legs, day, spent |
| `fee_cap_lowered` | max_fee_bps | tenant_ref | max_fee_bps |
| `paused`, `resumed` | by | tenant_ref | by |
| `revoked` | by, frozen | tenant_ref | by, frozen |
| `withdrawn` | to, amount | tenant_ref | to, amount |
| `swept` | by, amount | tenant_ref | by, amount |
| `token_recovered` | token, amount | tenant_ref | token, amount |
| `limits_updated` | total_budget, per_payment_cap, per_recipient_daily_cap, approval_threshold, payout_ttl_seconds, expires_at | tenant_ref | total_budget, per_payment_cap, per_recipient_daily_cap, approval_threshold, payout_ttl_seconds, expires_at |
| `role_added`, `role_removed` | role, account | tenant_ref | role, account |
| `approvals_required_set` | required | tenant_ref | required |
| `recovery_threshold_set` | required, recovery_epoch | tenant_ref | required, recovery_epoch |
| `recovery_vote_added` | kind, old, new, votes, required, recovery_epoch | tenant_ref | kind, old, new, votes, required, recovery_epoch |
| `recovery_executed` | kind, old, new, by | tenant_ref | kind, old, new, by |
| `recipient_allowed`, `recipient_disallowed` | recipient_ref, destination | tenant_ref | recipient_ref, destination |
| `attestation_policy_set` | attester, country_policy_hash, policy_epoch | tenant_ref | attester, country_policy_hash, policy_epoch |
| `principal_proposed` | current, proposed | tenant_ref | current, proposed |
| `principal_transferred` | previous, current | tenant_ref | previous, current |
| `principal_proposal_cancelled` | current, proposed | tenant_ref | current, proposed |
