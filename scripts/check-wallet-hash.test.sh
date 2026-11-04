#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Self test for check-wallet-hash.sh, with --no-build only, so it needs no
# network and no toolchain. Each case copies the real pins and fixtures into a
# throwaway root and changes one thing.
set -uo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
script="$root/scripts/check-wallet-hash.sh"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

passed=0
failures=0

# A copy of what the check reads, with the deployment record left out.
fresh() {
  local dir="$work/$1"
  mkdir -p "$dir/scripts" "$dir/soroban/contracts/wallet-factory" "$dir/deployments"
  cp "$root/scripts/build-smart-account.sh" "$dir/scripts/"
  cp -r "$root/soroban/contracts/wallet-factory/fixtures" "$dir/soroban/contracts/wallet-factory/"
  echo "$dir"
}

expect() {
  local name=$1 want=$2 needle=$3 dir=$4 out code=0
  out=$(ROOT="$dir" "$script" --no-build 2>&1) || code=$?
  if [ "$code" -ne "$want" ]; then
    echo "FAIL: $name: exit $code, want $want"
    printf '%s\n' "$out" | sed 's/^/    /'
    failures=$((failures + 1))
  elif ! grep -qF -- "$needle" <<<"$out"; then
    echo "FAIL: $name: output does not mention '$needle'"
    printf '%s\n' "$out" | sed 's/^/    /'
    failures=$((failures + 1))
  else
    echo "ok: $name"
    passed=$((passed + 1))
  fi
}

pin() {
  sed -n "s/^ *pin_$1=\([0-9a-f]\{64\}\)$/\1/p" "$root/scripts/build-smart-account.sh" | head -n 1
}

d=$(fresh good)
expect "the committed fixtures match the pins" 0 "ok: fixture multisig_account_example.wasm" "$d"

d=$(fresh tampered)
printf '\0' >>"$d/soroban/contracts/wallet-factory/fixtures/multisig_account_example.wasm"
expect "a changed fixture is refused" 1 "fixture multisig_account_example.wasm is" "$d"

d=$(fresh missing)
rm "$d/soroban/contracts/wallet-factory/fixtures/multisig_weighted_threshold_policy_example.wasm"
expect "a missing fixture is refused" 2 "missing fixture" "$d"

d=$(fresh unpinned)
sed -i 's/^ *pin_verifier=.*/    pin_verifier=""/' "$d/scripts/build-smart-account.sh"
expect "an empty pin is refused" 2 "no pinned verifier hash" "$d"

d=$(fresh no-wallet-record)
printf '{"network": "testnet"}\n' >"$d/deployments/testnet.json"
expect "a record without a wallet factory passes" 0 "ok: fixture" "$d"

d=$(fresh good-record)
printf '{"wallet_factory": {"account_wasm_hash": "%s", "verifier_wasm_hash": "%s", "policy_wasm_hash": "%s"}}\n' \
  "$(pin account)" "$(pin verifier)" "$(pin policy)" >"$d/deployments/testnet.json"
expect "a record with the pinned hashes passes" 0 "ok: recorded account_wasm_hash" "$d"

d=$(fresh bad-record)
printf '{"wallet_factory": {"account_wasm_hash": "%s", "verifier_wasm_hash": "%s", "policy_wasm_hash": "%s"}}\n' \
  "$(printf 'ab%.0s' $(seq 32))" "$(pin verifier)" "$(pin policy)" >"$d/deployments/testnet.json"
expect "a record with another account hash is refused" 1 "recorded account_wasm_hash is" "$d"

out=$("$script" --frobnicate 2>&1)
code=$?
if [ "$code" -eq 2 ] && grep -qF "usage:" <<<"$out"; then
  echo "ok: unknown option"
  passed=$((passed + 1))
else
  echo "FAIL: unknown option: exit $code"
  failures=$((failures + 1))
fi

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
