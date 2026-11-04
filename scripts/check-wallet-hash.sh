#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# The passkey wallet factory must deploy exactly the smart account code that
# was rebuilt from the pinned upstream tag and nothing else. Checks that one
# set of hashes runs through every place that holds it:
#   - the account, verifier and policy rebuilt from the tag, which
#     scripts/build-smart-account.sh already fails if they differ from its pins;
#   - the wasm the wallet factory's tests load, under
#     soroban/contracts/wallet-factory/fixtures/;
#   - the hashes a deployment record holds for a wallet factory, when
#     deployments/testnet.json has one.
#
# --no-build skips the rebuild and checks the fixtures and the record against
# the pins only, which needs no network and no toolchain. --out-dir leaves the
# rebuilt wasm there.
#
# ROOT overrides the repository root, only for the self test.
#
# Usage: scripts/check-wallet-hash.sh [--no-build] [--out-dir DIR]
# Exit codes: 0 every hash matches, 1 a hash differs (named on stderr), 2 bad
# usage, a missing file or a failed build.
set -euo pipefail

root=${ROOT:-"$(cd "$(dirname "$0")/.." && pwd)"}
tag=v0.8.0

usage() {
  echo "usage: scripts/check-wallet-hash.sh [--no-build] [--out-dir DIR]" >&2
  exit 2
}

build=1
out_args=()
while [ "$#" -gt 0 ]; do
  case "$1" in
    --no-build)
      build=0
      shift
      ;;
    --out-dir)
      [ "$#" -ge 2 ] || usage
      out_args=(--out-dir "$2")
      shift 2
      ;;
    *) usage ;;
  esac
done

script="$root/scripts/build-smart-account.sh"
fixtures="$root/soroban/contracts/wallet-factory/fixtures"
record="$root/deployments/testnet.json"

pin() {
  local value
  value=$(sed -n "s/^ *pin_$1=\([0-9a-f]\{64\}\)$/\1/p" "$script" | head -n 1)
  if [ -z "$value" ]; then
    echo "check-wallet-hash: no pinned $1 hash in scripts/build-smart-account.sh" >&2
    exit 2
  fi
  echo "$value"
}
account=$(pin account)
verifier=$(pin verifier)
policy=$(pin policy)

failed=0
compare() {
  local what=$1 got=$2 want=$3
  if [ "$got" = "$want" ]; then
    echo "ok: $what $got"
  else
    echo "check-wallet-hash: $what is $got, but $tag pins $want" >&2
    failed=1
  fi
}

if [ "$build" -eq 1 ]; then
  # Fails by itself on a hash that differs from its pin.
  "$script" --all "${out_args[@]}" "$tag" || exit "$?"
fi

for pair in "multisig_account_example:$account" \
  "multisig_webauthn_verifier_example:$verifier" \
  "multisig_weighted_threshold_policy_example:$policy"; do
  file="$fixtures/${pair%%:*}.wasm"
  if [ ! -f "$file" ]; then
    echo "check-wallet-hash: missing fixture ${file#"$root"/}" >&2
    exit 2
  fi
  compare "fixture ${pair%%:*}.wasm" "$(sha256sum "$file" | cut -d' ' -f1)" "${pair#*:}"
done

# A record that names no wallet factory yet has nothing to compare.
if [ -f "$record" ]; then
  hashes=$(python3 - "$record" <<'PY'
import json, sys
w = json.load(open(sys.argv[1], encoding="utf-8")).get("wallet_factory")
if w is not None:
    print(w.get("account_wasm_hash", ""), w.get("verifier_wasm_hash", ""), w.get("policy_wasm_hash", ""))
PY
  ) || exit 2
  if [ -n "$hashes" ]; then
    read -r rec_account rec_verifier rec_policy <<<"$hashes"
    compare "recorded account_wasm_hash" "$rec_account" "$account"
    compare "recorded verifier_wasm_hash" "$rec_verifier" "$verifier"
    compare "recorded policy_wasm_hash" "$rec_policy" "$policy"
  fi
fi

exit "$failed"
