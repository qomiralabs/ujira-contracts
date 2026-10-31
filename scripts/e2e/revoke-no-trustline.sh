#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Revoke for a principal that cannot receive the asset, end to end on
# testnet, against the factory in deployments/testnet.json.
#
# Revoke must never depend on a token transfer, so it has to land even when
# the principal holds no trustline, and the money must stay in the mandate
# until the principal can take it. The script checks, on the network:
#
#   1. The principal holds no trustline for the asset (any left from an
#      earlier run is emptied back to the issuer and removed first).
#   2. A mandate created through the factory and funded by the issuer holds
#      the funds.
#   3. revoke lands as simulated, with no extra resources, returns `frozen`
#      equal to the balance, and the transaction carries no event from the
#      asset contract, so no transfer was made. The balance is unchanged.
#   4. withdraw fails with TRANSFER_FAILED (contract error 72) and the
#      balance is unchanged.
#   5. Once the principal opens a trustline, withdraw succeeds and the
#      principal holds the whole balance.
#
# Usage: scripts/e2e/revoke-no-trustline.sh
# Exit codes: 0 every check passed, 1 otherwise.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
# shellcheck source=scripts/testnet/lib.sh
. "$root/scripts/testnet/lib.sh"

record="$root/deployments/testnet.json"
[ -f "$record" ] || testnet_die "no deployments/testnet.json; run scripts/testnet/deploy.sh first"
python3 "$root/scripts/testnet/check-deployment.py" "$record" >/dev/null ||
  testnet_die "deployments/testnet.json failed its check"

testnet_init "$root"

factory=$(testnet_json "$record" 'd["factory"]')
pick='[a for a in d["supported_assets"] if a.get("test_only")][0]'
asset=$(testnet_json "$record" "${pick}[\"contract\"]")
asset_code=$(testnet_json "$record" "${pick}[\"code\"]")
asset_issuer=$(testnet_json "$record" "${pick}[\"issuer\"]")
line="$asset_code:$asset_issuer"
[ "$(testnet_account ujira-testnet-issuer)" = "$asset_issuer" ] ||
  testnet_die "the local issuer identity does not issue $line; rerun the deploy"

principal=$(testnet_account ujira-testnet-nt-principal)
testnet_log "factory $factory, asset $asset ($line), principal $principal"

# 1. No trustline. Stellar removes a trustline only at a zero balance.
held=$(testnet_trustline_balance "$principal" "$asset_code" "$asset_issuer")
if [ "$held" != none ]; then
  if [ "$held" -gt 0 ]; then
    testnet_tx reset_return_balance st tx new payment --source-account ujira-testnet-nt-principal \
      --destination "$asset_issuer" --asset "$line" --amount "$held" "${TESTNET_NET[@]}" >/dev/null
  fi
  testnet_tx reset_remove_trustline st tx new change-trust --source-account ujira-testnet-nt-principal \
    --line "$line" --limit 0 "${TESTNET_NET[@]}" >/dev/null
fi
testnet_expect "principal trustline before the test" none \
  "$(testnet_trustline_balance "$principal" "$asset_code" "$asset_issuer")"

balance_of() {
  testnet_read --id "$asset" --source-account ujira-testnet-nt-principal -- balance --id "$1" | testnet_unquote
}

# 2. A mandate with no roles at all: only the principal acts on it. With no
# approvers, the approval threshold must sit above the payment cap.
unit=10000000
amount=$((25 * unit))
now=$(testnet_now)
config=$(
  cat <<JSON
{
  "principal": "$principal",
  "asset": "$asset",
  "recipient_mode": 1,
  "tenant_ref": "$(testnet_hex32)",
  "agents": [],
  "approvers": [],
  "approvals_required": 0,
  "guardians": [],
  "executors": [],
  "recovery": [],
  "recovery_threshold": 0,
  "limits": {
    "total_budget": "$((100 * unit))",
    "per_payment_cap": "$((10 * unit))",
    "per_recipient_daily_cap": "$((10 * unit))",
    "approval_threshold": "$((10 * unit + 1))",
    "payout_ttl_seconds": 3600,
    "expires_at": $((now + 7 * 86400))
  },
  "attester": null,
  "country_policy_hash": "$(printf '0%.0s' $(seq 64))",
  "max_fee_bps": 0
}
JSON
)
mandate=$(testnet_tx create_mandate st contract invoke --id "$factory" \
  --source-account ujira-testnet-nt-principal "${TESTNET_NET[@]}" -- \
  create --config "$config" --salt "$(testnet_hex32)" | testnet_unquote)
testnet_log "mandate $mandate"

# The principal cannot hold the asset, so the issuer funds the mandate.
testnet_tx fund_mandate st contract invoke --id "$asset" --source-account ujira-testnet-issuer \
  "${TESTNET_NET[@]}" -- mint --to "$mandate" --amount "$amount" >/dev/null
testnet_expect "mandate balance after funding" "$amount" "$(balance_of "$mandate")"

# 3. Revoke, exactly as simulated.
frozen=$(testnet_tx revoke st contract invoke --id "$mandate" --source-account ujira-testnet-nt-principal \
  "${TESTNET_NET[@]}" -- revoke | testnet_unquote)
revoke_tx=$(tail -n 1 "$TESTNET_RUN/txs.tsv" | cut -f2)
testnet_expect "revoke frozen equals the balance" "$amount" "$frozen"
testnet_expect "mandate balance after revoke" "$amount" "$(balance_of "$mandate")"
testnet_expect "principal trustline after revoke" none \
  "$(testnet_trustline_balance "$principal" "$asset_code" "$asset_issuer")"

# The asset contract emits an event for every transfer, so a revoke whose
# transaction carries no contract event from it made no transfer. The
# diagnostic events name every call into the asset contract; the only one
# allowed is the balance read that sets `frozen`. The mandate's own revoked
# event must be there with that amount, which also proves the events were
# read at all.
testnet_retry st tx fetch events --hash "$revoke_tx" "${TESTNET_NET[@]}" --output json \
  >"$TESTNET_RUN/revoke.events.json"
python3 - "$TESTNET_RUN/revoke.events.json" "$asset" "$mandate" "$amount" <<'PY' || testnet_die "revoke events are wrong"
import base64, json, sys
path, asset, mandate, amount = sys.argv[1:]
events = json.load(open(path))
asset_hex = base64.b32decode(asset)[1:33].hex()

contract = [e for group in events.get("contract_events", []) for e in group]
if any(e.get("contract_id") == asset for e in contract):
    sys.exit("the revoke transaction carries a contract event from the asset contract")
revoked = [
    e for e in contract
    if e.get("contract_id") == mandate
    and e["body"]["v0"]["topics"][0] == {"symbol": "revoked"}
]
if len(revoked) != 1:
    sys.exit("the revoke transaction carries no single revoked event from the mandate")
data = {kv["key"]["symbol"]: kv["val"] for kv in revoked[0]["body"]["v0"]["data"]["map"]}
if data.get("frozen") != {"i128": amount}:
    sys.exit(f"the revoked event has frozen {data.get('frozen')}, not {amount}")

called = set()
for d in events.get("diagnostic_events", []):
    topics = d["event"]["body"]["v0"]["topics"]
    if topics and topics[0] == {"symbol": "fn_call"} and topics[1] == {"bytes": asset_hex}:
        called.add(topics[2]["symbol"])
if called != {"balance"}:
    sys.exit(f"the revoke called the asset contract with {sorted(called)}, not only balance")
PY
testnet_log "ok: revoke emitted revoked { frozen: $amount }, no asset event, and only read the asset balance"

# 4. Withdraw cannot pay a principal without a trustline. The CLI stops at
# simulation, so nothing is sent and nothing changes.
err="$TESTNET_RUN/withdraw_no_trustline.err"
if testnet_retry st contract invoke --id "$mandate" --source-account ujira-testnet-nt-principal \
  "${TESTNET_NET[@]}" -- withdraw --amount "$amount" >/dev/null 2>"$err"; then
  testnet_die "withdraw succeeded without a trustline"
fi
grep -q 'Error(Contract, #72)' "$err" || {
  cat "$err" >&2
  testnet_die "withdraw failed, but not with TRANSFER_FAILED (#72)"
}
testnet_log "ok: withdraw without a trustline fails with Error(Contract, #72), TRANSFER_FAILED"
testnet_expect "mandate balance after the failed withdraw" "$amount" "$(balance_of "$mandate")"

# 5. With a trustline the same withdraw goes through.
testnet_trust principal_trustline ujira-testnet-nt-principal "$line" >/dev/null
testnet_tx withdraw st contract invoke --id "$mandate" --source-account ujira-testnet-nt-principal \
  "${TESTNET_NET[@]}" -- withdraw --amount "$amount" >/dev/null
testnet_expect "mandate balance after withdraw" 0 "$(balance_of "$mandate")"
testnet_expect "principal balance after withdraw" "$amount" \
  "$(testnet_trustline_balance "$principal" "$asset_code" "$asset_issuer")"

testnet_log "revoke without a trustline passed on mandate $mandate"
echo "Mandate: $mandate"
testnet_print_txs
