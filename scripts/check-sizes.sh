#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Fails when a Celo contract outgrows its bytecode budget.
#
# Budgets (runtime code, in bytes):
#   factories  61,440   Celo's 65,536 limit minus a 4 KiB margin
#   all others 24,576   the EIP-170 limit, kept as our own budget so the
#                       mandate stays cheap to deploy and portable
#
# A contract counts as a factory when its name ends in "Factory". Everything
# else, the mandate included, gets the tighter budget, so a misnamed contract
# can only ever be held to the stricter limit. Initcode is checked against
# Celo's 131,072 byte limit for every contract.
#
# Sizes come from `forge build --sizes --json`, which lists the contracts in
# src/. forge refuses anything over 24,576 bytes on its own, whatever
# code_size_limit says, so that one refusal is tolerated and our budgets are
# applied instead. Any other forge failure stops the check.
#
# Needs forge and jq.
#
# Usage: scripts/check-sizes.sh [celo dir]
#        scripts/check-sizes.sh --from-json <file>   (sizes already captured)
# Exit codes: 0 all within budget, 1 one or more contracts over budget,
# 2 sizes could not be produced or read.
set -euo pipefail

mandate_budget=24576
factory_budget=61440
initcode_limit=131072

json_file=""
if [ "${1:-}" = "--from-json" ]; then
  if [ $# -lt 2 ]; then
    echo "check-sizes: --from-json needs a file" >&2
    exit 2
  fi
  json_file=$2
  shift 2
fi
celo=${1:-"$(cd "$(dirname "$0")/.." && pwd)/celo"}

if ! command -v jq >/dev/null 2>&1; then
  echo "check-sizes: jq is not installed" >&2
  exit 2
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

if [ -z "$json_file" ]; then
  if ! command -v forge >/dev/null 2>&1; then
    echo "check-sizes: forge is not installed" >&2
    exit 2
  fi
  if [ ! -f "$celo/foundry.toml" ]; then
    echo "check-sizes: no foundry.toml in $celo" >&2
    exit 2
  fi

  # A plain build first, so a compile error is never mistaken for forge's
  # size refusal below.
  if ! (cd "$celo" && forge build) >"$tmp/build.log" 2>&1; then
    cat "$tmp/build.log" >&2
    echo "check-sizes: forge build failed in $celo" >&2
    exit 2
  fi

  # Celo's initcode limit is not EIP-3860's, so forge's own initcode check is
  # switched off and replaced by the one below.
  code=0
  (cd "$celo" && forge build --sizes --json --ignore-eip-3860) >"$tmp/sizes.json" 2>"$tmp/sizes.err" || code=$?
  if [ "$code" -ne 0 ] && ! grep -q 'exceed the runtime size limit' "$tmp/sizes.err"; then
    cat "$tmp/sizes.err" >&2
    echo "check-sizes: forge build --sizes failed in $celo" >&2
    exit 2
  fi
  json_file="$tmp/sizes.json"
fi

# An empty or reshaped report would otherwise pass with nothing checked. Sizes
# must be whole, non-negative and small enough for shell arithmetic: a failed
# comparison in the loop below would otherwise read as within budget.
if ! jq -e 'def size: type == "number" and . >= 0 and . == floor and . < 2147483648;
    type == "object" and length > 0
    and all(.[]; (.runtime_size | size) and (.init_size | size))' \
  "$json_file" >/dev/null 2>&1; then
  echo "check-sizes: could not read contract sizes from $json_file" >&2
  exit 2
fi

checked=0
failed=0
while IFS=$'\t' read -r key runtime init; do
  # jq can print a valid number in a form the shell cannot compare, so check
  # again here rather than trust the comparison to fail closed.
  if ! [[ $runtime =~ ^[0-9]{1,10}$ && $init =~ ^[0-9]{1,10}$ ]]; then
    echo "check-sizes: could not read contract sizes for $key" >&2
    exit 2
  fi
  name=${key##*:}
  case "$name" in
    *Factory) kind=factory budget=$factory_budget ;;
    *) kind=contract budget=$mandate_budget ;;
  esac
  checked=$((checked + 1))

  if [ "$runtime" -gt "$budget" ]; then
    echo "over budget: $name ($kind) runtime $runtime bytes, budget $budget" >&2
    failed=$((failed + 1))
  elif [ "$init" -gt "$initcode_limit" ]; then
    echo "over budget: $name ($kind) initcode $init bytes, Celo limit $initcode_limit" >&2
    failed=$((failed + 1))
  else
    echo "ok: $name ($kind) runtime $runtime of $budget bytes, initcode $init"
  fi
done < <(jq -r 'to_entries[] | [.key, .value.runtime_size, .value.init_size] | @tsv' "$json_file")

if [ "$failed" -gt 0 ]; then
  echo "check-sizes: $failed of $checked contract(s) over budget" >&2
  exit 1
fi

echo "check-sizes: $checked contract(s) within budget"
