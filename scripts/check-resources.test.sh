#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Self test for check-resources.py. Each case compares two small snapshot files
# and checks the exit code and that the output names the scenario and the metric,
# so a case cannot pass by failing for some other reason. Nothing is built here;
# the CI job runs the harness against the real contracts.
set -euo pipefail

script="$(cd "$(dirname "$0")" && pwd)/check-resources.py"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

passed=0
failures=0

# snapshot <file> <instructions> <memory> <read entries> <write entries> <read bytes> <write bytes>
snapshot() {
  printf '{ "scenarios": { "execute_batch_max": { "instructions": %s, "memory_bytes": %s, "read_entries": %s, "write_entries": %s, "read_bytes": %s, "write_bytes": %s } } }\n' \
    "$2" "$3" "$4" "$5" "$6" "$7" >"$work/$1"
}

printf '{ "margin_percent": 2 }\n' >"$work/budget.json"
snapshot committed.json 1000000 200000 100 50 10000 5000

# expect <case name> <exit code> <fresh file> [text the output must contain]...
expect() {
  local name=$1 want=$2 fresh=$3 out code=0 needle
  shift 3
  out=$("$script" --fresh "$work/$fresh" --snapshot "$work/${snapshot_file:-committed.json}" --budget "$work/${budget_file:-budget.json}" 2>&1) || code=$?
  if [ "$code" -ne "$want" ]; then
    echo "FAIL: $name: exit $code, want $want"
    printf '%s\n' "$out" | sed 's/^/    /'
    failures=$((failures + 1))
    return
  fi
  for needle in "$@"; do
    if ! grep -qF -- "$needle" <<<"$out"; then
      echo "FAIL: $name: the output does not mention '$needle'"
      printf '%s\n' "$out" | sed 's/^/    /'
      failures=$((failures + 1))
      return
    fi
  done
  echo "ok: $name"
  passed=$((passed + 1))
}

snapshot same.json 1000000 200000 100 50 10000 5000
expect "an unchanged snapshot passes" 0 same.json "1 scenario(s), 6 metric(s) within the 2 percent margin"

snapshot at_margin.json 1020000 204000 102 51 10200 5100
expect "exactly at the margin passes" 0 at_margin.json

snapshot under_margin.json 980000 196000 98 49 9800 4900
expect "exactly at the margin below passes" 0 under_margin.json

snapshot over.json 1020001 200000 100 50 10000 5000
expect "instructions over margin" 1 over.json \
  "execute_batch_max: instructions 1000000 to 1020001" "a regression beyond the 2 percent margin" "--update"

snapshot over_memory.json 1000000 204001 100 50 10000 5000
expect "memory over margin" 1 over_memory.json "execute_batch_max: memory_bytes 200000 to 204001"

snapshot over_reads.json 1000000 200000 103 50 10000 5000
expect "read entries over margin" 1 over_reads.json "execute_batch_max: read_entries 100 to 103"

snapshot over_writes.json 1000000 200000 100 52 10000 5000
expect "an extra storage write fails" 1 over_writes.json "execute_batch_max: write_entries 50 to 52"

snapshot over_read_bytes.json 1000000 200000 100 50 10201 5000
expect "read bytes over margin" 1 over_read_bytes.json "execute_batch_max: read_bytes 10000 to 10201"

snapshot over_write_bytes.json 1000000 200000 100 50 10000 5101
expect "write bytes over margin" 1 over_write_bytes.json "execute_batch_max: write_bytes 5000 to 5101"

snapshot improved.json 979999 200000 100 50 10000 5000
expect "an improvement beyond the margin fails until the snapshot is updated" 1 improved.json \
  "execute_batch_max: instructions 1000000 to 979999" "an improvement beyond the 2 percent margin"

snapshot every.json 2000000 400000 200 100 20000 10000
expect "every metric is named in one run" 1 every.json "6 difference(s)" "instructions" "memory_bytes" "read_entries" "write_entries" "read_bytes" "write_bytes"

snapshot zero.json 1000000 200000 100 0 10000 5000
snapshot zero_after.json 1000000 200000 100 1 10000 5000
snapshot_file=zero.json expect "any change from zero fails" 1 zero_after.json "write_entries 0 to 1" "from zero"
snapshot_file=zero.json expect "zero that stays zero passes" 0 zero.json

printf '{ "scenarios": { "a_new_call": { "instructions": 1, "memory_bytes": 1, "read_entries": 1, "write_entries": 1, "read_bytes": 1, "write_bytes": 1 }, "execute_batch_max": { "instructions": 1000000, "memory_bytes": 200000, "read_entries": 100, "write_entries": 50, "read_bytes": 10000, "write_bytes": 5000 } } }\n' >"$work/extra.json"
expect "a scenario missing from the snapshot fails" 1 extra.json "a_new_call: in the fresh run but not in the snapshot"
snapshot_file=extra.json expect "a scenario missing from the fresh run fails" 1 same.json "a_new_call: in the snapshot but not in the fresh run"

printf '{ "margin_percent": 50 }\n' >"$work/wide.json"
budget_file=wide.json expect "the margin comes from the budget file" 0 over.json "within the 50 percent margin"

printf '{ "margin_percent": 2.5 }\n' >"$work/fraction.json"
budget_file=fraction.json expect "a margin that is not whole exits 2" 2 same.json "must hold a whole margin_percent"
budget_file=none.json expect "a missing budget file exits 2" 2 same.json "cannot read"
expect "a missing fresh file exits 2" 2 none.json "cannot read"
printf 'not json\n' >"$work/bad.json"
expect "a fresh file that is not JSON exits 2" 2 bad.json "is not valid JSON"
printf '{ "scenarios": {} }\n' >"$work/empty.json"
expect "a snapshot with no scenarios exits 2" 2 empty.json "has no scenarios"
printf '{ "scenarios": { "x": { "instructions": 1 } } }\n' >"$work/short.json"
expect "a scenario missing a metric exits 2" 2 short.json "scenario x must have exactly"
snapshot fractional.json 1000000.5 200000 100 50 10000 5000
expect "a metric that is not whole exits 2" 2 fractional.json "instructions is not a whole number"
printf '{ "scenarios": { "x": { "instructions": 1, "instructions": 2 } } }\n' >"$work/repeat.json"
expect "a repeated key exits 2" 2 repeat.json "appears twice in one object"

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
