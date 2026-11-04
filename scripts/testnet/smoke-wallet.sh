#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Smoke test of the passkey wallet factory on Stellar testnet, against the
# deployment deployments/testnet.json records:
#   1. deploy a 2 of 3 passkey wallet through the wallet factory, and check its
#      address equals predict_address and its code is the pinned account wasm;
#   2. create a mandate from the mandate factory with that wallet as principal,
#      the wallet authorizing create with two WebAuthn signatures;
#   3. read the mandate back and check its principal is the wallet.
#
# The passkeys come from scripts/testnet/wallet_auth.py, whose secrets are
# public by construction, so the wallet protects nothing and is never funded.
# Every run uses a fresh name, so it can be repeated.
#
# Usage: scripts/testnet/smoke-wallet.sh
# Needs stellar, python3 with the cryptography package, curl and git.
# Exit codes: 0 every check passed, 1 any step failed.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
# shellcheck source=scripts/testnet/lib.sh
. "$here/lib.sh"

record="$root/deployments/testnet.json"
auth="$here/wallet_auth.py"
testnet_init "$root"

field() {
  testnet_json "$record" "$1"
}
wallet_factory=$(field 'd["wallet_factory"]["factory"]')
verifier=$(field 'd["wallet_factory"]["verifier"]')
policy=$(field 'd["wallet_factory"]["policy"]')
mandate_factory=$(field 'd["factory"]')
asset=$(field '[a["contract"] for a in d["supported_assets"] if a["code"] == "USDC"][0]')
account_pin=$(sed -n 's/^ *pin_account=\([0-9a-f]\{64\}\)$/\1/p' "$root/scripts/build-smart-account.sh")
[ -n "$account_pin" ] || testnet_die "no pinned account hash in scripts/build-smart-account.sh"
testnet_account ujira-testnet-deployer >/dev/null

name="testnet-smoke-$(date -u +%Y%m%dT%H%M%SZ)"
salt=$(python3 -c 'import hashlib,sys; print(hashlib.sha256(sys.argv[1].encode()).hexdigest())' "$name")
signers=$(python3 "$auth" signers "$name" 3 "$verifier")
policies=$(python3 "$auth" policies "$name" 3 2 "$verifier" "$policy")
testnet_log "wallet $name: 3 passkeys, threshold 2"

# 1. The wallet.
read_wallet_factory() {
  testnet_read --id "$wallet_factory" --source-account ujira-testnet-deployer -- "$@"
}
predicted=$(read_wallet_factory predict_address --signers "$signers" --policies "$policies" --salt "$salt" | testnet_unquote)
wallet=$(testnet_tx deploy_wallet st contract invoke --id "$wallet_factory" \
  --source-account ujira-testnet-deployer "${TESTNET_NET[@]}" -- \
  deploy --signers "$signers" --policies "$policies" --salt "$salt" | testnet_unquote)
testnet_expect "wallet address equals predict_address" "$predicted" "$wallet"
st contract fetch --id "$wallet" "${TESTNET_NET[@]}" --out-file "$TESTNET_RUN/wallet.wasm" >/dev/null ||
  testnet_die "cannot fetch the wallet's code"
testnet_expect "wallet wasm hash" "$account_pin" "$(sha256sum "$TESTNET_RUN/wallet.wasm" | cut -d' ' -f1)"

# 2. The mandate, with the wallet as principal. The CLI cannot sign for a
# contract account, so the transaction is built and simulated, the wallet's
# authorization entry is signed with two passkeys, and the transaction is
# simulated again so its resources cover the wallet's own checks.
unit=10000000
now=$(testnet_now)
mandate_salt=$(testnet_hex32)
config=$(
  cat <<JSON
{
  "principal": "$wallet",
  "asset": "$asset",
  "recipient_mode": 1,
  "tenant_tag": "$(testnet_hex32)",
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
tx="$TESTNET_RUN/create"
testnet_retry st contract invoke --id "$mandate_factory" --source-account ujira-testnet-deployer \
  "${TESTNET_NET[@]}" --build-only -- create --config "$config" --salt "$mandate_salt" >"$tx.built" ||
  testnet_die "cannot build create"
testnet_retry st tx simulate --source-account ujira-testnet-deployer "${TESTNET_NET[@]}" <"$tx.built" >"$tx.recorded" ||
  testnet_die "cannot simulate create"
latest=$(st ledger latest "${TESTNET_NET[@]}" --output json | testnet_json - 'd["sequence"]') ||
  testnet_die "cannot read the latest ledger"
st tx decode <"$tx.recorded" |
  python3 "$auth" sign "$name" 3 2 "$verifier" "$wallet" "$((latest + 100))" "$TESTNET_PASSPHRASE" |
  st tx encode >"$tx.authorized" || testnet_die "cannot sign the wallet's authorization"
testnet_retry st tx simulate --source-account ujira-testnet-deployer "${TESTNET_NET[@]}" --auth-mode enforce <"$tx.authorized" >"$tx.assembled" ||
  testnet_die "create does not simulate with the wallet's signatures"
st tx sign --sign-with-key ujira-testnet-deployer "${TESTNET_NET[@]}" <"$tx.assembled" >"$tx.signed" ||
  testnet_die "cannot sign the transaction"
testnet_retry st tx send "${TESTNET_NET[@]}" <"$tx.signed" >"$tx.sent" 2>"$tx.err" || {
  cat "$tx.err" >&2
  testnet_die "create with the wallet as principal failed"
}
create_tx=$(st tx hash "${TESTNET_NET[@]}" <"$tx.signed")
printf '%s\t%s\n' create_mandate "$create_tx" >>"$TESTNET_RUN/txs.tsv"
testnet_log "create_mandate: $create_tx"

# 3. The mandate's address follows from the factory, the principal and the
# salt; read it back.
deployer_salt=$(python3 -c 'import hashlib,sys; print(hashlib.sha256(bytes.fromhex(sys.argv[1] + sys.argv[2])).hexdigest())' \
  "$(python3 "$auth" address-xdr "$wallet")" "$mandate_salt")
mandate=$(python3 "$auth" contract-id "$mandate_factory" "$deployer_salt" "$TESTNET_PASSPHRASE")
principal=$(testnet_read --id "$mandate" --source-account ujira-testnet-deployer -- config |
  testnet_json - 'd["principal"]')
testnet_expect "mandate $mandate principal" "$wallet" "$principal"

testnet_log "wallet $wallet and mandate $mandate"
testnet_print_txs
