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
# A contract gets the factory budget only when it is listed, by source path and
# name, in the committed allowlist (celo/factory-allowlist.txt by default, one
# `src/<file>.sol:<Contract>` per line, `#` starts a comment). Its name plays
# no part, so renaming a mandate to ...Factory changes nothing. Everything
# else, the mandate included, gets the tighter budget. An allowlist entry that
# is malformed, listed twice or matches no contract is an error. Initcode is
# checked against Celo's 131,072 byte limit for every contract.
#
# forge names a contract by its bare name when the name is unique and as
# `Name (path)` otherwise. In forge mode a bare name is resolved to its source
# path from the build artifacts. A bare name in a --from-json report has no
# path, so it is always held to the mandate budget.
#
# Sizes come from `forge build --sizes --json`, run with test/ and script/
# skipped. Test helpers such as mocks are never deployed and would otherwise be
# reported too. What remains is every contract in src/ plus every concrete
# vendored contract under lib/ that src/ compiles, whether src/ deploys it,
# calls it or only imports it. forge refuses anything over
# 24,576 bytes on its own, whatever code_size_limit says, so that one refusal
# is tolerated and our budgets are applied instead. Any other forge failure
# stops the check.
#
# Needs forge and jq.
#
# Usage: scripts/check-sizes.sh [--factories <file>] [celo dir]
#        scripts/check-sizes.sh [--factories <file>] --from-json <file>   (sizes already captured)
# Exit codes: 0 all within budget, 1 one or more contracts over budget,
# 2 sizes could not be produced or read.
set -euo pipefail

mandate_budget=24576
factory_budget=61440
initcode_limit=131072

json_file=""
factories_file=""
while [ $# -gt 0 ]; do
  case "$1" in
    --from-json | --factories)
      if [ $# -lt 2 ]; then
        echo "check-sizes: $1 needs a file" >&2
        exit 2
      fi
      if [ "$1" = "--from-json" ]; then json_file=$2; else factories_file=$2; fi
      shift 2
      ;;
    *) break ;;
  esac
done
case "${1:-}" in
  -*)
    echo "check-sizes: unknown option $1" >&2
    exit 2
    ;;
esac
if [ $# -gt 1 ]; then
  echo "check-sizes: unexpected argument after the celo dir: $2 (options go first)" >&2
  exit 2
fi
celo=${1:-"$(cd "$(dirname "$0")/.." && pwd)/celo"}
factories_file=${factories_file:-"$celo/factory-allowlist.txt"}

if ! command -v jq >/dev/null 2>&1; then
  echo "check-sizes: jq is not installed" >&2
  exit 2
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# The allowlist, one validated entry per line in $tmp/factories.
if [ ! -f "$factories_file" ]; then
  echo "check-sizes: no factory allowlist at $factories_file" >&2
  exit 2
fi
if [ ! -r "$factories_file" ]; then
  echo "check-sizes: cannot read the factory allowlist at $factories_file" >&2
  exit 2
fi
# bash 3.2 cuts a line at a NUL byte and bash 5 drops it, so a control
# character could turn a rejected entry into an accepted one depending on the
# shell. Check the raw bytes before reading lines: only tab, line feed and
# carriage return are allowed.
control=$(LC_ALL=C tr -d '\011\012\015\040-\176\200-\377' <"$factories_file" | wc -c)
if [ "$(printf '%s' "$control" | tr -d ' ')" != 0 ]; then
  echo "check-sizes: $factories_file contains a NUL byte or another control character" >&2
  exit 2
fi
# src/<dirs>/<file>.sol:<Contract>, where a Solidity identifier may use $.
entry_pattern='^src/([A-Za-z0-9_.-]+/)*[A-Za-z0-9_-][A-Za-z0-9_.-]*\.sol:[A-Za-z_$][A-Za-z0-9_$]*$'
: >"$tmp/factories"
while IFS= read -r line || [ -n "$line" ]; do
  line=${line%%#*}
  line=$(printf '%s' "$line" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')
  [ -z "$line" ] && continue
  if ! [[ $line =~ $entry_pattern ]] ||
    [[ $line == *"/../"* || $line == *"/./"* ]]; then
    echo "check-sizes: allowlist entry '$line' is not of the form src/<file>.sol:<Contract>" >&2
    exit 2
  fi
  if grep -qxF -- "$line" "$tmp/factories"; then
    echo "check-sizes: allowlist entry $line is listed twice" >&2
    exit 2
  fi
  printf '%s\n' "$line" >>"$tmp/factories"
done <"$factories_file"

from_forge=""
if [ -z "$json_file" ]; then
  from_forge=1
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
  (cd "$celo" && forge build --sizes --json --ignore-eip-3860 --skip './test/**' './script/**') \
    >"$tmp/sizes.json" 2>"$tmp/sizes.err" || code=$?
  if [ "$code" -ne 0 ] && ! grep -q 'exceed the runtime size limit' "$tmp/sizes.err"; then
    cat "$tmp/sizes.err" >&2
    echo "check-sizes: forge build --sizes failed in $celo" >&2
    exit 2
  fi
  json_file="$tmp/sizes.json"

  # Bare names in the report carry no path; map each contract to the file it
  # was compiled from, as `name<TAB>path`. Every concrete vendored contract
  # under lib/ that src/ compiles is in the report too and resolves here; only
  # src/ paths can be allowlisted, so they always get the mandate budget.
  out_dir="$celo/out"
  if [ -d "$out_dir" ]; then
    find "$out_dir" -name '*.json' -not -path '*/build-info/*' -print0 |
      xargs -0 jq -r '(.metadata.settings.compilationTarget // {}) | to_entries[]
        | select(.key | (startswith("test/") or startswith("script/")) | not)
        | "\(.value)\t\(.key)"' 2>/dev/null |
      sort -u >"$tmp/sources" || true
  fi
fi
touch "$tmp/sources"

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
: >"$tmp/used"
while IFS=$'\t' read -r key runtime init; do
  # jq can print a valid number in a form the shell cannot compare, so check
  # again here rather than trust the comparison to fail closed.
  if ! [[ $runtime =~ ^[0-9]{1,10}$ && $init =~ ^[0-9]{1,10}$ ]]; then
    echo "check-sizes: could not read contract sizes for $key" >&2
    exit 2
  fi
  # `Name (path)` from forge, `path:Name`, or a bare name.
  path=""
  if [[ $key =~ ^([^ ]+)\ \((.+)\)$ ]]; then
    name=${BASH_REMATCH[1]} path=${BASH_REMATCH[2]}
  elif [[ $key == *:* ]]; then
    name=${key##*:} path=${key%:*}
  else
    name=$key
    if [ -n "$from_forge" ]; then
      matches=$(awk -F '\t' -v n="$name" '$1 == n { print $2 }' "$tmp/sources")
      if [ "$(printf '%s' "$matches" | grep -c .)" -ne 1 ]; then
        echo "check-sizes: could not find the source file of $name in the build artifacts" >&2
        exit 2
      fi
      path=$matches
    fi
  fi
  shown=$name
  kind=contract budget=$mandate_budget
  if [ -n "$path" ]; then
    shown="$path:$name"
    if grep -qxF -- "$shown" "$tmp/factories"; then
      kind=factory budget=$factory_budget
      printf '%s\n' "$shown" >>"$tmp/used"
    fi
  fi
  checked=$((checked + 1))

  if [ "$runtime" -gt "$budget" ]; then
    echo "over budget: $shown ($kind) runtime $runtime bytes, budget $budget" >&2
    failed=$((failed + 1))
  elif [ "$init" -gt "$initcode_limit" ]; then
    echo "over budget: $shown ($kind) initcode $init bytes, Celo limit $initcode_limit" >&2
    failed=$((failed + 1))
  else
    echo "ok: $shown ($kind) runtime $runtime of $budget bytes, initcode $init"
  fi
done < <(jq -r 'to_entries[] | [.key, .value.runtime_size, .value.init_size] | @tsv' "$json_file")

# An entry for a contract that no longer exists would quietly hand the factory
# budget to whatever later appears at that path.
unused=0
while IFS= read -r entry; do
  if ! grep -qxF -- "$entry" "$tmp/used"; then
    echo "check-sizes: allowlist entry $entry matches no contract" >&2
    unused=$((unused + 1))
  fi
done <"$tmp/factories"
if [ "$unused" -gt 0 ]; then
  exit 2
fi

if [ "$failed" -gt 0 ]; then
  echo "check-sizes: $failed of $checked contract(s) over budget" >&2
  exit 1
fi

echo "check-sizes: $checked contract(s) within budget"
