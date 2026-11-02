#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Self test for check-wasm-sizes.sh. Each case builds a throwaway git repo with a
# budget file on a base commit, and a release folder of files of exact sizes, and
# checks the exit code and that the output names the problem, so a case cannot
# pass by failing for some other reason. Nothing is compiled.
set -euo pipefail

script="$(cd "$(dirname "$0")" && pwd)/check-wasm-sizes.sh"
work=$(cd "$(mktemp -d)" && pwd -P)
trap 'rm -rf "$work"' EXIT

passed=0
failures=0

g() {
  git -C "$repo" -c core.hooksPath=/dev/null -c user.name=Test -c user.email=test@example.com "$@"
}

# sized <contract> <bytes>: a release wasm of exactly that size
sized() {
  mkdir -p "$repo/release"
  head -c "$2" /dev/zero >"$repo/release/$1.wasm"
}

budgets() {
  printf '%s\n' "$1" >"$repo/wasm-budgets.json"
}

# new_repo <name> <budget json on the base commit>: the base commit, with the same
# budget file left in place for the case to change or keep
new_repo() {
  repo="$work/$1"
  mkdir -p "$repo"
  git -C "$repo" init -q -b main
  printf '{ "constants": { "SOROBAN_MAX_WASM_BYTES": 65536 } }\n' >"$repo/constants.json"
  budgets "$2"
  g add -A
  g commit -q -m 'chore: base'
  g commit -q --allow-empty -m 'chore: head'
}

# expect <case name> <exit code> [text the output must contain]...
expect() {
  local name=$1 want=$2 out code=0 needle
  shift 2
  out=$("$script" --no-build --base "${base_ref:-HEAD^1}" --release-dir "$repo/release" \
    --budgets "$repo/wasm-budgets.json" --constants "$repo/constants.json" 2>&1) || code=$?
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

one='{ "ujira_mandate": { "budget": 60000, "recorded": 50000 } }'

new_repo within "$one"
sized ujira_mandate 50000
expect "a contract within its limits passes" 0 \
  "ujira_mandate: 50000 bytes, budget 60000, limit 65536, recorded on base 50000" \
  "1 contract(s) within budget and limit"

new_repo over_limit '{ "ujira_mandate": { "budget": 65536, "recorded": 65000 } }'
sized ujira_mandate 65537
expect "wasm above the network limit fails" 1 "ujira_mandate is 65537 bytes, above the network limit of 65536 bytes"

new_repo at_limit '{ "ujira_mandate": { "budget": 65536, "recorded": 65000 } }'
sized ujira_mandate 65536
expect "wasm exactly at the network limit passes" 0

new_repo over_budget "$one"
sized ujira_mandate 54000
budgets '{ "ujira_mandate": { "budget": 53999, "recorded": 50000 } }'
expect "wasm above its budget fails" 1 "ujira_mandate is 54000 bytes, above its budget of 53999 bytes"

new_repo at_budget "$one"
sized ujira_mandate 54000
budgets '{ "ujira_mandate": { "budget": 54000, "recorded": 50000 } }'
expect "wasm exactly at its budget passes" 0

new_repo budget_over_limit "$one"
sized ujira_mandate 50000
budgets '{ "ujira_mandate": { "budget": 70000, "recorded": 50000 } }'
expect "a budget above the network limit fails" 1 "has budget 70000, above the network limit"

new_repo growth "$one"
sized ujira_mandate 55500
expect "11 percent growth without a budget change fails" 1 \
  "ujira_mandate grew from 50000 to 55500 bytes, more than 10 percent"

new_repo growth_changed "$one"
sized ujira_mandate 55500
budgets '{ "ujira_mandate": { "budget": 60000, "recorded": 55500 } }'
expect "11 percent growth with a budget change passes" 0

new_repo growth_exact "$one"
sized ujira_mandate 55000
expect "exactly 10 percent growth passes" 0

new_repo growth_just_over "$one"
sized ujira_mandate 55001
expect "one byte over 10 percent fails" 1 "grew from 50000 to 55001 bytes"

new_repo shrink "$one"
sized ujira_mandate 30000
expect "a smaller contract passes" 0

new_repo committed_change "$one"
sized ujira_mandate 55500
budgets '{ "ujira_mandate": { "budget": 60000, "recorded": 55500 } }'
g add -A
g commit -q -m 'chore: budget'
base_ref=HEAD~2 expect "a budget change already committed on the branch counts" 0

new_repo new_contract "$one"
sized ujira_mandate 50000
sized ujira_other 100
expect "a wasm with no budget fails" 1 "ujira_other has no budget"

new_repo new_contract_ok "$one"
sized ujira_mandate 50000
sized ujira_other 100
budgets '{ "ujira_mandate": { "budget": 60000, "recorded": 50000 },
  "ujira_other": { "budget": 200, "recorded": 100 } }'
expect "a new contract with a budget has nothing to grow from" 0 "ujira_other: 100 bytes, budget 200, limit 65536, recorded on base none"

new_repo stale_budget "$one"
sized ujira_other 100
budgets '{ "ujira_mandate": { "budget": 60000, "recorded": 50000 }, "ujira_other": { "budget": 200, "recorded": 100 } }'
expect "a budget with no wasm fails" 1 "ujira_mandate is in" "has no wasm in the release folder"

new_repo empty_release "$one"
mkdir -p "$repo/release"
expect "an empty release folder fails" 1 "no wasm in"

new_repo no_release "$one"
expect "a missing release folder exits 2" 2 "no release folder at"

new_repo bad_base "$one"
sized ujira_mandate 50000
base_ref=no-such-ref expect "a base that does not resolve exits 2" 2 "cannot resolve the base commit no-such-ref"

new_repo not_base_tracked "$one"
sized ujira_mandate 50000
budgets '{ "ujira_mandate": { "budget": 60000, "recorded": 50000 } }'
g rm -q --cached wasm-budgets.json
g commit -q -m 'chore: untrack'
g commit -q --allow-empty -m 'chore: head'
expect "a budget file the base does not hold counts as changed" 0 "recorded on base none"

new_repo bad_budget_file "$one"
sized ujira_mandate 50000
budgets '{ "ujira_mandate": { "budget": "lots", "recorded": 50000 } }'
expect "a budget that is not a number exits 2" 2 "must map each contract to whole numbers"

new_repo fraction "$one"
sized ujira_mandate 50000
budgets '{ "ujira_mandate": { "budget": 60000.5, "recorded": 50000 } }'
expect "a budget that is not whole exits 2" 2 "must map each contract to whole numbers"

new_repo no_constant "$one"
sized ujira_mandate 50000
printf '{ "constants": {} }\n' >"$repo/constants.json"
expect "a missing limit constant exits 2" 2 "no integer SOROBAN_MAX_WASM_BYTES"

new_repo several '{ "a": { "budget": 10, "recorded": 10 }, "b": { "budget": 10, "recorded": 10 } }'
sized a 20
sized b 20
expect "every problem is named in one run" 1 "a is 20 bytes, above its budget" "b is 20 bytes, above its budget"

unknown_out=$("$script" --bogus 2>&1) && unknown_code=0 || unknown_code=$?
if [ "$unknown_code" -eq 2 ] && [[ "$unknown_out" == *"unknown option --bogus"* ]]; then
  echo "ok: an unknown option exits 2"
  passed=$((passed + 1))
else
  echo "FAIL: an unknown option: exit $unknown_code"
  failures=$((failures + 1))
fi

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
