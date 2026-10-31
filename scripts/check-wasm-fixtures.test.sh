#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Self test for check-wasm-fixtures.sh. Each case builds a throwaway git repo
# with fixture files, and a stub stellar binary that "builds" whatever bytes
# the case asks for, so no real build runs.
set -euo pipefail

script="$(cd "$(dirname "$0")" && pwd)/check-wasm-fixtures.sh"
work=$(cd "$(mktemp -d)" && pwd -P)
trap 'rm -rf "$work"' EXIT

# The stub writes $STUB_BYTES to <out-dir>/<package with _>.wasm, or fails
# when STUB_FAIL is set.
stub="$work/stellar"
cat >"$stub" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
[ -z "${STUB_FAIL:-}" ] || { echo "stub build failed" >&2; exit 1; }
while [ $# -gt 0 ]; do
  case "$1" in
    --package) package=$2; shift 2 ;;
    --out-dir) out=$2; shift 2 ;;
    *) shift ;;
  esac
done
printf '%s' "$STUB_BYTES" >"$out/${package//-/_}.wasm"
STUB
chmod +x "$stub"

passed=0
failures=0

new_repo() {
  repo="$work/$1"
  mkdir -p "$repo"
  git -C "$repo" init -q
}

# fixture <path> <content>, then stage it
fixture() {
  mkdir -p "$(dirname "$repo/$1")"
  printf '%s' "$2" >"$repo/$1"
  git -C "$repo" add -f -- "$1"
}

# expect <case name> <exit code> [text the output must contain]...
expect() {
  local name=$1 want=$2
  shift 2
  local out code=0 needle
  out=$(STELLAR="$stub" "$script" "$repo" 2>&1) || code=$?
  if [ "$code" -ne "$want" ]; then
    echo "FAIL: $name: exit $code, want $want"
    printf '%s\n' "$out" | sed 's/^/    /'
    failures=$((failures + 1))
    return
  fi
  for needle in "$@"; do
    if ! printf '%s\n' "$out" | grep -qF -- "$needle"; then
      echo "FAIL: $name: output does not mention '$needle'"
      printf '%s\n' "$out" | sed 's/^/    /'
      failures=$((failures + 1))
      return
    fi
  done
  echo "ok: $name"
  passed=$((passed + 1))
}

new_repo empty
expect "no fixtures passes" 0 "0 fixture(s) checked"

new_repo match
fixture soroban/spikes/probe/fixtures/probe_child.wasm "same bytes"
STUB_BYTES="same bytes" expect "fixture that matches its build" 0 "1 fixture(s) checked"

new_repo differ
fixture soroban/spikes/probe/fixtures/probe_child.wasm "old bytes"
STUB_BYTES="new bytes" expect "fixture that drifted" 1 "soroban/spikes/probe/fixtures/probe_child.wasm" "probe-child"

new_repo runner
fixture soroban/vector-runner/fixtures/probe_token.wasm "old bytes"
STUB_BYTES="new bytes" expect "vector runner fixture that drifted" 1 "soroban/vector-runner/fixtures/probe_token.wasm" "probe-token"

new_repo build_fails
fixture soroban/spikes/probe/fixtures/probe_child.wasm "bytes"
STUB_FAIL=1 STUB_BYTES="bytes" expect "build failure" 2 "stub build failed"

new_repo untracked
mkdir -p "$repo/soroban/spikes/probe/fixtures"
printf 'x' >"$repo/soroban/spikes/probe/fixtures/stray.wasm"
expect "untracked wasm is ignored" 0 "0 fixture(s) checked"

expect_not_repo() {
  local out code=0
  out=$(STELLAR="$stub" "$script" "$work/nowhere" 2>&1) || code=$?
  if [ "$code" -eq 2 ]; then
    echo "ok: not a work tree"
    passed=$((passed + 1))
  else
    echo "FAIL: not a work tree: exit $code, want 2"
    failures=$((failures + 1))
  fi
}
mkdir -p "$work/nowhere"
expect_not_repo

echo "check-wasm-fixtures.test: $passed passed, $failures failed"
[ "$failures" -eq 0 ]
