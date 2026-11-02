#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Self test for check-caches.py. Each case writes a fixture workflow to a temp
# directory and checks the exit code and that the output names the problem, so a
# case cannot pass by failing for some other reason. The fixture lines hold workflow
# expressions, which single quotes keep literal on purpose.
# shellcheck disable=SC2016
set -euo pipefail

script="$(cd "$(dirname "$0")" && pwd)/check-caches.py"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

passed=0
failures=0

good_key='soroban-${{ runner.os }}-${{ github.ref }}-${{ hashFiles('"'"'soroban/Cargo.lock'"'"', '"'"'soroban/rust-toolchain.toml'"'"') }}'

# step <lines of a step under "steps:">: writes a workflow holding it
workflow() {
  local name=$1
  shift
  {
    printf 'name: CI\non: pull_request\njobs:\n  build:\n    runs-on: ubuntu-24.04\n    steps:\n'
    printf '      - name: Check out\n        uses: actions/checkout@abc # v1\n'
    printf '%s\n' "$@"
    printf '      - name: Build\n        run: cargo build\n'
  } >"$work/$name"
}

# expect <case name> <exit code> <workflow file> [text the output must contain]...
expect() {
  local name=$1 want=$2 file=$3 out code=0 needle
  shift 3
  out=$("$script" "$work/$file" 2>&1) || code=$?
  if [ "$code" -ne "$want" ]; then
    echo "FAIL: $name: exit $code, want $want"
    printf '%s\n' "$out" | sed 's/^/    /'
    failures=$((failures + 1))
    return
  fi
  for needle in "$@"; do
    if [[ "$out" != *"$needle"* ]]; then
      echo "FAIL: $name: output does not mention '$needle'"
      printf '%s\n' "$out" | sed 's/^/    /'
      failures=$((failures + 1))
      return
    fi
  done
  echo "ok: $name"
  passed=$((passed + 1))
}

workflow exact.yml \
  '      - name: Cache cargo' \
  '        uses: actions/cache@abc # v6' \
  '        with:' \
  '          path: |' \
  '            ~/.cargo/registry/cache' \
  '            soroban/target' \
  "          key: $good_key"
expect "an exact key passes" 0 exact.yml "every target cache has an exact key"

workflow restore_keys.yml \
  '      - name: Cache cargo' \
  '        uses: actions/cache@abc # v6' \
  '        with:' \
  '          path: |' \
  '            soroban/target' \
  "          key: $good_key" \
  '          restore-keys: |' \
  '            soroban-${{ runner.os }}-'
expect "restore-keys for soroban/target fails" 1 restore_keys.yml "restore-keys for soroban/target" "restore_keys.yml:10:"

workflow restore_action.yml \
  '      - uses: actions/cache/restore@abc' \
  '        with:' \
  '          path: soroban/target' \
  "          key: $good_key" \
  '          restore-keys: soroban-'
expect "restore-keys on the restore action fails" 1 restore_action.yml "restore-keys for soroban/target"

workflow other_target.yml \
  '      - uses: actions/cache@abc' \
  '        with:' \
  '          path: spikes/thing/target/release' \
  '          key: thing' \
  '          restore-keys: thing-'
expect "any other target directory is held to the same rule" 1 other_target.yml "restore-keys for spikes/thing/target/release"

workflow not_target.yml \
  '      - uses: actions/cache@abc' \
  '        with:' \
  '          path: ~/.npm' \
  '          key: npm-${{ runner.os }}' \
  '          restore-keys: npm-'
expect "restore-keys on a cache with no target directory passes" 0 not_target.yml

workflow word_target.yml \
  '      - uses: actions/cache@abc' \
  '        with:' \
  '          path: docs/targets-and-limits' \
  '          key: docs' \
  '          restore-keys: docs-'
expect "a path that merely contains the word passes" 0 word_target.yml

workflow no_ref.yml \
  '      - uses: actions/cache@abc' \
  '        with:' \
  '          path: soroban/target' \
  "          key: soroban-\${{ hashFiles('soroban/Cargo.lock', 'soroban/rust-toolchain.toml') }}"
expect "a key without the ref fails" 1 no_ref.yml "does not name github.ref"

workflow no_lock.yml \
  '      - uses: actions/cache@abc' \
  '        with:' \
  '          path: soroban/target' \
  "          key: soroban-\${{ github.ref }}-\${{ hashFiles('soroban/rust-toolchain.toml') }}"
expect "a key without the lockfile fails" 1 no_lock.yml "does not name Cargo.lock"

workflow no_toolchain.yml \
  '      - uses: actions/cache@abc' \
  '        with:' \
  '          path: soroban/target' \
  "          key: soroban-\${{ github.ref }}-\${{ hashFiles('soroban/Cargo.lock') }}"
expect "a key without the toolchain file fails" 1 no_toolchain.yml "does not name rust-toolchain.toml"

workflow no_key.yml \
  '      - uses: actions/cache@abc' \
  '        with:' \
  '          path: soroban/target'
expect "a target cache with no key fails" 1 no_key.yml "no key for soroban/target"

workflow path_expression.yml \
  '      - uses: actions/cache@abc' \
  '        with:' \
  '          path: ${{ env.CACHE_PATH }}' \
  '          key: anything'
expect "a path from an expression fails" 1 path_expression.yml "the cache path is an expression"

workflow rust_cache.yml \
  '      - uses: Swatinem/rust-cache@abc'
expect "an action that keeps its own keys fails" 1 rust_cache.yml "keeps its own keys"

workflow release.yml \
  '      - uses: actions/cache@abc' \
  '        with:' \
  '          path: ~/.npm' \
  '          key: npm'
expect "any cache in a release workflow fails" 1 release.yml "in a release or deploy workflow"

workflow deploy-testnet.yml \
  '      - uses: actions/cache@abc' \
  '        with:' \
  '          path: soroban/target' \
  "          key: $good_key"
expect "an exact target cache in a deploy workflow still fails" 1 deploy-testnet.yml "in a release or deploy workflow"

workflow release_setup.yml \
  '      - uses: actions/setup-node@abc' \
  '        with:' \
  '          node-version: 24' \
  '          cache: npm'
mv "$work/release_setup.yml" "$work/release-setup.yml"
expect "a setup action cache in a release workflow fails" 1 release-setup.yml "has a cache input"

workflow release_clean.yml \
  '      - uses: actions/setup-node@abc' \
  '        with:' \
  '          node-version: 24'
mv "$work/release_clean.yml" "$work/release-clean.yml"
expect "a release workflow with no cache passes" 0 release-clean.yml

expect "a missing file exits 2" 2 no_such.yml "cannot read"

# The real workflows.
real=$("$script" 2>&1) && real_code=0 || real_code=$?
if [ "$real_code" -eq 0 ]; then
  echo "ok: the real workflows pass"
  passed=$((passed + 1))
else
  echo "FAIL: the real workflows: exit $real_code"
  printf '%s\n' "$real" | sed 's/^/    /'
  failures=$((failures + 1))
fi

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
