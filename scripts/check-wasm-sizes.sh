#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# The network refuses to upload a contract above its size limit, and nobody
# should find that out at deploy time. Builds the production contracts with
# scripts/soroban-build.sh, prints each one's size, budget and limit, and fails if
# any contract:
#   - is above the network limit, SOROBAN_MAX_WASM_BYTES in spec/constants.json;
#   - is above its own budget in soroban/wasm-budgets.json, set with headroom
#     below that limit;
#   - grew more than 10 percent over the size recorded in the budget file on the
#     base commit, unless the budget file changes in the same pull request.
#
# The budget file must list every wasm in the release folder and nothing else, so
# a new contract cannot arrive without a budget. Sizes are whole bytes and the
# arithmetic is integer.
#
# Needs jq and git, and what soroban-build.sh needs when building.
#
# Usage: scripts/check-wasm-sizes.sh [--no-build] [--base REF] [--release-dir DIR]
#        [--budgets FILE] [--constants FILE]
#   --no-build     check what is already in the release folder
#   --base REF     the commit to compare the budget file with (default HEAD^1,
#                  the base branch on a pull request's merge commit)
# Exit codes: 0 every size is within its limits, 1 a size or budget problem, named
# on stderr, 2 bad usage or a file or ref that cannot be read.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
build=1
base=HEAD^1
release_dir="$root/soroban/target/wasm32v1-none/release"
budgets="$root/soroban/wasm-budgets.json"
constants="$root/spec/constants.json"

die() {
  echo "check-wasm-sizes: $1" >&2
  exit 2
}

while [ $# -gt 0 ]; do
  case $1 in
    --no-build) build=0 ;;
    --base | --release-dir | --budgets | --constants)
      [ $# -ge 2 ] || die "$1 needs a value"
      case $1 in
        --base) base=$2 ;;
        --release-dir) release_dir=$2 ;;
        --budgets) budgets=$2 ;;
        --constants) constants=$2 ;;
      esac
      shift
      ;;
    *) die "unknown option $1" ;;
  esac
  shift
done

command -v jq >/dev/null 2>&1 || die "jq is not installed"

if [ "$build" -eq 1 ]; then
  "$root/scripts/soroban-build.sh" || exit 1
fi

[ -f "$constants" ] || die "cannot read $constants"
[ -f "$budgets" ] || die "cannot read $budgets"
[ -d "$release_dir" ] || die "no release folder at $release_dir; build the contracts first"

limit=$(jq -r '.constants.SOROBAN_MAX_WASM_BYTES // empty' "$constants") || die "$constants is not valid JSON"
case $limit in
  '' | *[!0-9]*) die "$constants has no integer SOROBAN_MAX_WASM_BYTES" ;;
esac

jq -e 'type == "object" and all(.[]; (.budget | type) == "number" and (.recorded | type) == "number"
  and .budget == (.budget | floor) and .recorded == (.recorded | floor))' "$budgets" >/dev/null ||
  die "$budgets must map each contract to whole numbers { budget, recorded }"

# The budget file on the base commit, compared by content. A path outside any
# repository, or a ref that does not resolve, fails rather than passing.
budget_dir=$(cd "$(dirname "$budgets")" && pwd)
budget_name=$(basename "$budgets")
git -C "$budget_dir" rev-parse --verify --quiet "$base^{commit}" >/dev/null ||
  die "cannot resolve the base commit $base; a shallow checkout needs fetch-depth 2"
budget_changed=1
if base_budgets=$(git -C "$budget_dir" show "$base:./$budget_name" 2>/dev/null); then
  if [ "$base_budgets" = "$(cat "$budgets")" ]; then
    budget_changed=0
  fi
else
  base_budgets='{}'
fi

failed=0
problem() {
  echo "check-wasm-sizes: $1" >&2
  failed=1
}

listed=$(jq -r 'keys[]' "$budgets")
found=0
for wasm in "$release_dir"/*.wasm; do
  [ -e "$wasm" ] || continue
  found=$((found + 1))
  name=$(basename "$wasm" .wasm)
  size=$(wc -c <"$wasm" | tr -d ' ')
  budget=$(jq -r --arg n "$name" '.[$n].budget // empty' "$budgets")
  recorded=$(printf '%s' "$base_budgets" | jq -r --arg n "$name" '.[$n].recorded // empty')
  if [ -z "$budget" ]; then
    echo "$name: $size bytes, no budget, limit $limit"
    problem "$name has no budget in $budgets"
  else
    echo "$name: $size bytes, budget $budget, limit $limit, recorded on base ${recorded:-none}"
    if [ "$budget" -gt "$limit" ]; then
      problem "$name has budget $budget, above the network limit of $limit bytes"
    fi
    if [ "$size" -gt "$budget" ]; then
      problem "$name is $size bytes, above its budget of $budget bytes"
    fi
  fi
  if [ "$size" -gt "$limit" ]; then
    problem "$name is $size bytes, above the network limit of $limit bytes"
  fi
  # Whole number arithmetic: more than 10 percent means 10 * growth > recorded.
  if [ -n "$recorded" ] && [ "$budget_changed" -eq 0 ] && [ $((10 * (size - recorded))) -gt "$recorded" ]; then
    problem "$name grew from $recorded to $size bytes, more than 10 percent; change $budget_name in the same pull request if this is intended"
  fi
done

[ "$found" -gt 0 ] || problem "no wasm in $release_dir"

while IFS= read -r name; do
  [ -n "$name" ] || continue
  [ -e "$release_dir/$name.wasm" ] || problem "$name is in $budgets but has no wasm in the release folder"
done <<<"$listed"

if [ "$failed" -ne 0 ]; then
  exit 1
fi
echo "check-wasm-sizes: $found contract(s) within budget and limit"
