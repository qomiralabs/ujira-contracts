#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Smoke test of the factory recorded in deployments/testnet.json, on
# testnet. Walks one mandate through its life and checks the network state
# after every step:
#
#   create through the factory, fund it from the principal, allow a
#   recipient, request a payout as the agent (at the approval threshold, so
#   it waits), approve it, execute it to a recipient with a trustline, pause
#   and resume, revoke, and withdraw everything back to the principal.
#
# Amounts are integer minor units of the test asset (7 decimal places).
# Every run creates a new mandate under a fresh salt, so it can be repeated.
# The transaction hashes are printed at the end and kept with the run log.
#
# Usage: scripts/testnet/smoke.sh
# Exit codes: 0 every step landed and checked out, 1 otherwise.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
# shellcheck source=scripts/testnet/lib.sh
. "$here/lib.sh"

record="$root/deployments/testnet.json"
[ -f "$record" ] || testnet_die "no deployments/testnet.json; run scripts/testnet/deploy.sh first"
python3 "$here/check-deployment.py" "$record" >/dev/null || testnet_die "deployments/testnet.json failed its check"

testnet_init "$root"

factory=$(testnet_json "$record" 'd["factory"]')
asset=$(testnet_json "$record" '[a for a in d["supported_assets"] if a.get("test_only")][0]["contract"]')
asset_code=$(testnet_json "$record" '[a for a in d["supported_assets"] if a.get("test_only")][0]["code"]')
asset_issuer=$(testnet_json "$record" '[a for a in d["supported_assets"] if a.get("test_only")][0]["issuer"]')
line="$asset_code:$asset_issuer"
[ "$(testnet_account ujira-testnet-issuer)" = "$asset_issuer" ] ||
  testnet_die "the local issuer identity does not issue $line; rerun the deploy"

principal=$(testnet_account ujira-testnet-smoke-principal)
agent=$(testnet_account ujira-testnet-smoke-agent)
approver=$(testnet_account ujira-testnet-smoke-approver)
recipient=$(testnet_account ujira-testnet-smoke-recipient)
testnet_log "factory $factory, asset $asset ($line)"
testnet_log "principal $principal, agent $agent, approver $approver, recipient $recipient"

for who in smoke-principal smoke-recipient; do
  pk=$(st keys public-key "ujira-testnet-$who")
  if [ "$(testnet_trustline_balance "$pk" "$asset_code" "$asset_issuer")" = none ]; then
    testnet_trust "trust_${who#smoke-}" "ujira-testnet-$who" "$line" >/dev/null
  fi
done

balance_of() {
  testnet_read --id "$asset" --source-account ujira-testnet-smoke-principal -- balance --id "$1" | testnet_unquote
}
mandate_state() {
  testnet_read --id "$mandate" --source-account ujira-testnet-smoke-principal -- state
}

# 1 000 units funded, a 1 000 unit budget, payments up to 100 units, 200 a
# day per recipient, and anything from 10 units up needs one approval.
unit=10000000
fund=$((1000 * unit))
amount=$((50 * unit))
now=$(testnet_now)
# A test mandate has no tenant, so random bytes stand in for its tag.
tenant_tag=$(testnet_hex32)
salt=$(testnet_hex32)
config=$(
  cat <<JSON
{
  "principal": "$principal",
  "asset": "$asset",
  "recipient_mode": 1,
  "tenant_tag": "$tenant_tag",
  "agents": ["$agent"],
  "approvers": ["$approver"],
  "approvals_required": 1,
  "guardians": [],
  "executors": [],
  "recovery": [],
  "recovery_threshold": 0,
  "limits": {
    "total_budget": "$((1000 * unit))",
    "per_payment_cap": "$((100 * unit))",
    "per_recipient_daily_cap": "$((200 * unit))",
    "approval_threshold": "$((10 * unit))",
    "payout_ttl_seconds": 86400,
    "expires_at": $((now + 30 * 86400))
  },
  "attester": null,
  "country_policy_hash": "$(printf '0%.0s' $(seq 64))",
  "max_fee_bps": 100
}
JSON
)

mandate=$(testnet_tx create_mandate st contract invoke --id "$factory" \
  --source-account ujira-testnet-smoke-principal "${TESTNET_NET[@]}" -- \
  create --config "$config" --salt "$salt" | testnet_unquote)
testnet_log "mandate $mandate"
testnet_expect "mandate principal" "$principal" \
  "$(testnet_read --id "$mandate" --source-account ujira-testnet-smoke-principal -- config | testnet_json - 'd["principal"]')"

# The principal funds the mandate from its own trustline, as a real one would.
testnet_tx mint_to_principal st contract invoke --id "$asset" --source-account ujira-testnet-issuer \
  "${TESTNET_NET[@]}" -- mint --to "$principal" --amount "$fund" >/dev/null
testnet_tx fund_mandate st contract invoke --id "$asset" --source-account ujira-testnet-smoke-principal \
  "${TESTNET_NET[@]}" -- transfer --from "$principal" --to "$mandate" --amount "$fund" >/dev/null
testnet_expect "mandate balance after funding" "$fund" "$(balance_of "$mandate")"

recipient_ref=$(testnet_hex32)
testnet_tx allow_recipient st contract invoke --id "$mandate" --source-account ujira-testnet-smoke-principal \
  "${TESTNET_NET[@]}" -- allow_recipients \
  --entries "[{\"recipient_ref\":\"$recipient_ref\",\"destination\":\"$recipient\"}]" >/dev/null

payout_id=$(testnet_hex32)
request=$(
  cat <<JSON
{
  "payout_id": "$payout_id",
  "recipient_ref": "$recipient_ref",
  "total": "$amount",
  "fee": "0",
  "legs": [{"destination": "$recipient", "amount": "$amount"}],
  "deadline": $(($(testnet_now) + 3600)),
  "task_ref": "$(testnet_hex32)",
  "attestations": []
}
JSON
)
status=$(testnet_tx request_payout st contract invoke --id "$mandate" --source-account ujira-testnet-smoke-agent \
  "${TESTNET_NET[@]}" -- request_payout --caller "$agent" --req "$request")
testnet_expect "payout status after request (1 pending)" 1 "$status"

status=$(testnet_tx approve_payout st contract invoke --id "$mandate" --source-account ujira-testnet-smoke-approver \
  "${TESTNET_NET[@]}" -- approve --caller "$approver" --payout_id "$payout_id")
testnet_expect "payout status after approval (2 approved)" 2 "$status"

before=$(balance_of "$recipient")
testnet_tx execute_payout st contract invoke --id "$mandate" --source-account ujira-testnet-smoke-agent \
  "${TESTNET_NET[@]}" -- execute --caller "$agent" --payout_id "$payout_id" >/dev/null
testnet_expect "recipient received" "$((before + amount))" "$(balance_of "$recipient")"
testnet_expect "mandate spent" "$amount" "$(mandate_state | testnet_json - 'd["spent"]')"
left=$((fund - amount))
testnet_expect "mandate balance after execute" "$left" "$(balance_of "$mandate")"

testnet_tx pause st contract invoke --id "$mandate" --source-account ujira-testnet-smoke-principal \
  "${TESTNET_NET[@]}" -- pause --caller "$principal" >/dev/null
testnet_expect "paused" True "$(mandate_state | testnet_json - 'd["paused"]')"
testnet_tx resume st contract invoke --id "$mandate" --source-account ujira-testnet-smoke-principal \
  "${TESTNET_NET[@]}" -- resume >/dev/null
testnet_expect "paused after resume" False "$(mandate_state | testnet_json - 'd["paused"]')"

frozen=$(testnet_tx revoke st contract invoke --id "$mandate" --source-account ujira-testnet-smoke-principal \
  "${TESTNET_NET[@]}" -- revoke | testnet_unquote)
testnet_expect "revoke frozen" "$left" "$frozen"
testnet_expect "revoked" True "$(mandate_state | testnet_json - 'd["revoked"]')"

before=$(balance_of "$principal")
testnet_tx withdraw st contract invoke --id "$mandate" --source-account ujira-testnet-smoke-principal \
  "${TESTNET_NET[@]}" -- withdraw --amount "$left" >/dev/null
testnet_expect "mandate balance after withdraw" 0 "$(balance_of "$mandate")"
testnet_expect "principal balance after withdraw" "$((before + left))" "$(balance_of "$principal")"

testnet_log "smoke test passed on mandate $mandate"
echo "Mandate: $mandate"
testnet_print_txs
