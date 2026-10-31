#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Deploys the mandate factory to Stellar testnet and records the result in
# deployments/testnet.json. Safe to run again: every run builds, uploads and
# deploys a fresh factory, and the file then names the new one. After a
# testnet reset it recreates the accounts and assets it needs.
#
# Steps:
#   1. Build ujira-mandate and ujira-factory with `stellar contract build`
#      from a soroban/ tree that matches HEAD, so the recorded commit is the
#      source of the wasm.
#   2. Make sure the throwaway deployer, fee and test asset issuer accounts
#      exist and are funded by friendbot.
#   3. Wrap the test asset and testnet USDC in their asset contracts, unless
#      they are already deployed, and give the fee account a trustline to
#      both so a fee can always be paid to it.
#   4. Upload the mandate wasm and check the network hash matches the local
#      file, then upload the factory wasm and deploy it. The factory's
#      constructor takes the mandate wasm hash, the supported assets and the
#      fee address, all fixed for its lifetime.
#   5. Read the factory back from the network and check every value, then
#      write deployments/testnet.json. Public keys, contract ids and hashes
#      only; scripts/testnet/check-deployment.py refuses anything else.
#
# Usage: scripts/testnet/deploy.sh
# Needs stellar, cargo (through stellar), python3, curl and git.
# Exit codes: 0 deployed and recorded, 1 any step failed.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
# shellcheck source=scripts/testnet/lib.sh
. "$here/lib.sh"

out_file="$root/deployments/testnet.json"

if ! git -C "$root" diff --quiet HEAD -- soroban ||
  [ -n "$(git -C "$root" ls-files --others --exclude-standard -- soroban)" ]; then
  testnet_die "soroban/ differs from HEAD; commit or stash first so the recorded commit built the wasm"
fi
commit=$(git -C "$root" rev-parse HEAD)
soroban_tree=$(git -C "$root" rev-parse HEAD:soroban)

testnet_init "$root"

# 1. Build.
release="$root/soroban/target/wasm32v1-none/release"
for package in ujira-mandate ujira-factory; do
  testnet_log "building $package"
  (cd "$root/soroban" && stellar contract build --locked --package "$package") >&2 ||
    testnet_die "stellar contract build failed for $package"
done
mandate_wasm="$release/ujira_mandate.wasm"
factory_wasm="$release/ujira_factory.wasm"
mandate_local=$(sha256sum "$mandate_wasm" | cut -d' ' -f1)
factory_local=$(sha256sum "$factory_wasm" | cut -d' ' -f1)

# 2. Accounts.
deployer=$(testnet_account ujira-testnet-deployer)
fee_address=$(testnet_account ujira-testnet-fee)
issuer=$(testnet_account ujira-testnet-issuer)
testnet_log "deployer $deployer, fee address $fee_address, test asset issuer $issuer"

# 3. Assets.
asset_contract() {
  local line=$1 id
  id=$(st contract id asset --asset "$line" "${TESTNET_NET[@]}") || testnet_die "cannot derive the contract id of $line"
  if ! testnet_read --id "$id" --source-account ujira-testnet-deployer -- name >/dev/null 2>&1; then
    TESTNET_HASH_OPTIONAL=1 testnet_tx "wrap_${line%%:*}" st contract asset deploy --asset "$line" \
      --source-account ujira-testnet-deployer "${TESTNET_NET[@]}" >/dev/null
  fi
  echo "$id"
}
test_line="$TEST_ASSET_CODE:$issuer"
usdc_line="$TESTNET_USDC_CODE:$TESTNET_USDC_ISSUER"
test_asset=$(asset_contract "$test_line")
usdc_asset=$(asset_contract "$usdc_line")
for line in "$test_line" "$usdc_line"; do
  if [ "$(testnet_trustline_balance "$fee_address" "${line%%:*}" "${line#*:}")" = none ]; then
    testnet_trust "fee_trust_${line%%:*}" ujira-testnet-fee "$line" >/dev/null
  fi
done

# 4. Upload and deploy. The CLI optimizes uploads by default, which would
# change the bytes and so the hash; the network must hold exactly what was
# built.
upload() {
  local label=$1 wasm=$2 err="$TESTNET_RUN/$1.err" hash tx
  hash=$(testnet_retry st contract upload --wasm "$wasm" --optimize=false \
    --source-account ujira-testnet-deployer "${TESTNET_NET[@]}" 2>"$err") || {
    cat "$err" >&2
    testnet_die "$label failed"
  }
  # The CLI sends nothing when the network already holds this wasm.
  tx=$(testnet_hash_from "$err")
  if [ -n "$tx" ]; then
    printf '%s\t%s\n' "$label" "$tx" >>"$TESTNET_RUN/txs.tsv"
    testnet_log "$label: $tx"
  else
    testnet_log "$label: already on the network"
  fi
  echo "$hash"
}
mandate_hash=$(upload upload_mandate "$mandate_wasm")
testnet_expect "uploaded mandate wasm hash" "$mandate_local" "$mandate_hash"
factory_hash=$(upload upload_factory "$factory_wasm")
testnet_expect "uploaded factory wasm hash" "$factory_local" "$factory_hash"

assets_json="[\"$usdc_asset\",\"$test_asset\"]"
factory=$(testnet_tx deploy_factory st contract deploy --wasm-hash "$factory_hash" \
  --source-account ujira-testnet-deployer "${TESTNET_NET[@]}" -- \
  --mandate_wasm_hash "$mandate_hash" --assets "$assets_json" --fee_address "$fee_address")
case "$factory" in
  C?????????????????????????????????????????????????????*) ;;
  *) testnet_die "deploy returned '$factory', not a contract id" ;;
esac
deploy_tx=$(tail -n 1 "$TESTNET_RUN/txs.tsv" | cut -f2)

# 5. Check what the network holds, then record it.
read_factory() {
  testnet_read --id "$factory" --source-account ujira-testnet-deployer -- "$@"
}
testnet_expect "factory mandate_wasm_hash" "$mandate_hash" "$(read_factory mandate_wasm_hash | testnet_unquote)"
testnet_expect "factory fee_address" "$fee_address" "$(read_factory fee_address | testnet_unquote)"
testnet_expect "factory supported_assets" "$usdc_asset $test_asset" \
  "$(read_factory supported_assets | testnet_json - '" ".join(d)')"
spec_version=$(read_factory version)
testnet_expect "factory version" 1 "$spec_version"

mkdir -p "$(dirname "$out_file")"
tmp=$(mktemp)
python3 - "$tmp" <<PY
import json, sys
record = {
    "network": "testnet",
    "network_passphrase": "$TESTNET_PASSPHRASE",
    "factory": "$factory",
    "factory_wasm_hash": "$factory_hash",
    "mandate_wasm_hash": "$mandate_hash",
    "spec_version": int("$spec_version"),
    "supported_assets": [
        {"code": "$TESTNET_USDC_CODE", "issuer": "$TESTNET_USDC_ISSUER", "contract": "$usdc_asset"},
        {"code": "$TEST_ASSET_CODE", "issuer": "$issuer", "contract": "$test_asset", "test_only": True},
    ],
    "fee_address": "$fee_address",
    "deployer": "$deployer",
    "commit": "$commit",
    "soroban_tree": "$soroban_tree",
    "deploy_transaction": "$deploy_tx",
    "deployed_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
}
with open(sys.argv[1], "w") as f:
    json.dump(record, f, indent=2)
    f.write("\n")
PY
python3 "$here/check-deployment.py" "$tmp" || testnet_die "the deployment record failed its check"
mv "$tmp" "$out_file"
chmod 644 "$out_file"

testnet_log "factory $factory recorded in ${out_file#"$root"/}"
testnet_print_txs
