#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Deploys the passkey wallet factory to Stellar testnet, with the one
# WebAuthn verifier and the one weighted threshold policy every wallet names,
# and records them under "wallet_factory" in deployments/testnet.json. The
# mandate factory must be deployed first, by scripts/testnet/deploy.sh, because
# the wallet factory's deny list names it.
#
# Steps:
#   1. Check that the committed account, verifier and policy wasm are exactly
#      the pinned build (scripts/check-wallet-hash.sh --no-build), and build
#      ujira-wallet-factory from a soroban/ tree that matches HEAD.
#   2. Upload the three upstream wasm and the factory, and check every hash the
#      network reports against the local file.
#   3. Deploy one verifier and one policy, then the factory, with the account
#      hash and a deny list of every operator address this deployment knows:
#      the deployer, the fee address and the mandate factory.
#   4. Read the factory back, check it, and add "wallet_factory" to the record.
#      scripts/testnet/check-deployment.py refuses anything but public ids and
#      hashes.
#
# Usage: scripts/testnet/deploy-wallet-factory.sh
# Needs stellar, cargo (through stellar), python3, curl and git.
# Exit codes: 0 deployed and recorded, 1 any step failed.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
# shellcheck source=scripts/testnet/lib.sh
. "$here/lib.sh"

out_file="$root/deployments/testnet.json"
fixtures="$root/soroban/contracts/wallet-factory/fixtures"
tag=v0.8.0
commit_upstream=ddc327eb92d63709436da3a8455f641c2d5476c5

[ -f "$out_file" ] || testnet_die "no deployments/testnet.json; run scripts/testnet/deploy.sh first"
if ! git -C "$root" diff --quiet HEAD -- soroban ||
  [ -n "$(git -C "$root" ls-files --others --exclude-standard -- soroban)" ]; then
  testnet_die "soroban/ differs from HEAD; commit or stash first so the recorded commit built the wasm"
fi
commit=$(git -C "$root" rev-parse HEAD)

testnet_init "$root"

# 1. Check and build.
"$root/scripts/check-wallet-hash.sh" --no-build >&2 || testnet_die "the committed wasm is not the pinned build"
testnet_log "building ujira-wallet-factory"
(cd "$root/soroban" && stellar contract build --locked --package ujira-wallet-factory) >&2 ||
  testnet_die "stellar contract build failed for ujira-wallet-factory"
factory_wasm="$root/soroban/target/wasm32v1-none/release/ujira_wallet_factory.wasm"

read_record() {
  python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$out_file" "$1"
}
mandate_factory=$(read_record factory)
fee_address=$(read_record fee_address)
deployer=$(testnet_account ujira-testnet-deployer)
testnet_expect "deployer matches the record" "$(read_record deployer)" "$deployer"

# 2. Upload. The CLI optimizes uploads by default, which would change the
# bytes and so the hash; the network must hold exactly what was pinned.
upload() {
  local label=$1 wasm=$2 err="$TESTNET_RUN/$1.err" hash tx
  hash=$(testnet_retry st contract upload --wasm "$wasm" --optimize=false \
    --source-account ujira-testnet-deployer "${TESTNET_NET[@]}" 2>"$err") || {
    cat "$err" >&2
    testnet_die "$label failed"
  }
  tx=$(testnet_hash_from "$err")
  if [ -n "$tx" ]; then
    printf '%s\t%s\n' "$label" "$tx" >>"$TESTNET_RUN/txs.tsv"
    testnet_log "$label: $tx"
  else
    testnet_log "$label: already on the network"
  fi
  testnet_expect "$label hash" "$(sha256sum "$wasm" | cut -d' ' -f1)" "$hash"
  echo "$hash"
}
account_hash=$(upload upload_account "$fixtures/multisig_account_example.wasm")
verifier_hash=$(upload upload_verifier "$fixtures/multisig_webauthn_verifier_example.wasm")
policy_hash=$(upload upload_policy "$fixtures/multisig_weighted_threshold_policy_example.wasm")
factory_hash=$(upload upload_wallet_factory "$factory_wasm")

# 3. Deploy.
deploy() {
  local label=$1 hash=$2 id
  shift 2
  id=$(testnet_tx "$label" st contract deploy --wasm-hash "$hash" \
    --source-account ujira-testnet-deployer "${TESTNET_NET[@]}" "$@")
  case "$id" in
    C?????????????????????????????????????????????????????*) ;;
    *) testnet_die "$label returned '$id', not a contract id" ;;
  esac
  echo "$id"
}
verifier=$(deploy deploy_verifier "$verifier_hash")
policy=$(deploy deploy_policy "$policy_hash")
deny_json="[\"$deployer\",\"$fee_address\",\"$mandate_factory\"]"
factory=$(deploy deploy_wallet_factory "$factory_hash" -- \
  --account_wasm_hash "$account_hash" --deny_list "$deny_json")
deploy_tx=$(tail -n 1 "$TESTNET_RUN/txs.tsv" | cut -f2)

# 4. Check what the network holds, then record it.
read_factory() {
  testnet_read --id "$factory" --source-account ujira-testnet-deployer -- "$@"
}
testnet_expect "wallet factory account_wasm_hash" "$account_hash" "$(read_factory account_wasm_hash | testnet_unquote)"
testnet_expect "wallet factory deny_list" "$deployer $fee_address $mandate_factory" \
  "$(read_factory deny_list | testnet_json - '" ".join(d)')"

tmp=$(mktemp)
python3 - "$out_file" "$tmp" <<PY
import json, sys
record = json.load(open(sys.argv[1]))
record["wallet_factory"] = {
    "factory": "$factory",
    "factory_wasm_hash": "$factory_hash",
    "account_wasm_hash": "$account_hash",
    "verifier": "$verifier",
    "verifier_wasm_hash": "$verifier_hash",
    "policy": "$policy",
    "policy_wasm_hash": "$policy_hash",
    "deny_list": ["$deployer", "$fee_address", "$mandate_factory"],
    "upstream_tag": "$tag",
    "upstream_commit": "$commit_upstream",
    "commit": "$commit",
    "deploy_transaction": "$deploy_tx",
    "deployed_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
}
with open(sys.argv[2], "w") as f:
    json.dump(record, f, indent=2)
    f.write("\n")
PY
python3 "$here/check-deployment.py" "$tmp" || testnet_die "the deployment record failed its check"
mv "$tmp" "$out_file"
chmod 644 "$out_file"
"$root/scripts/check-wallet-hash.sh" --no-build >&2 || testnet_die "the recorded hashes are not the pinned build"

testnet_log "wallet factory $factory recorded in ${out_file#"$root"/}"
testnet_print_txs
