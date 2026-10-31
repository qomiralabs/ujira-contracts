#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Checks that every committed wasm fixture under soroban/spikes/*/fixtures/
# and soroban/vector-runner/fixtures/ is exactly what its source builds to
# today. Tests load these files because
# the deployer needs real wasm, so a fixture that drifted from its source
# would make the tests assert on code nobody can read.
#
# The package is named after the file: fixtures/soroban_edges_child.wasm is
# built from the soroban-edges-child package. Each package is built on its
# own into a temp dir with stellar contract build --locked, then compared
# byte for byte.
#
# STELLAR overrides the stellar binary (the self test uses a stub).
#
# Usage: scripts/check-wasm-fixtures.sh [path inside the repo]
# Exit codes: 0 every fixture matches, 1 one or more fixtures differ (named
# on stderr), 2 the tree could not be listed or a build failed.
set -euo pipefail

unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE

root=${1:-"$(cd "$(dirname "$0")/.." && pwd)"}
stellar=${STELLAR:-stellar}

if ! root=$(git -C "$root" rev-parse --show-toplevel 2>/dev/null); then
  echo "check-wasm-fixtures: ${1:-$root} is not inside a git work tree" >&2
  exit 2
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

if ! git -C "$root" ls-files -z -- 'soroban/spikes/*/fixtures/*.wasm' 'soroban/vector-runner/fixtures/*.wasm' >"$work/list"; then
  echo "check-wasm-fixtures: git ls-files failed in $root" >&2
  exit 2
fi

checked=0
failed=0
while IFS= read -r -d '' file; do
  checked=$((checked + 1))
  stem=$(basename "$file" .wasm)
  package=${stem//_/-}
  out="$work/$stem"
  mkdir -p "$out"
  if ! "$stellar" contract build --locked --manifest-path "$root/soroban/Cargo.toml" \
    --package "$package" --out-dir "$out" >"$work/build.log" 2>&1; then
    echo "check-wasm-fixtures: building $package for $file failed:" >&2
    sed 's/^/    /' "$work/build.log" >&2
    exit 2
  fi
  if [ ! -f "$out/$stem.wasm" ]; then
    echo "check-wasm-fixtures: building $package did not produce $stem.wasm" >&2
    exit 2
  fi
  if ! cmp -s "$out/$stem.wasm" "$root/$file"; then
    echo "fixture differs from its source: $file (rebuild with stellar contract build --package $package and copy it)" >&2
    failed=$((failed + 1))
  fi
done <"$work/list"

if [ "$failed" -gt 0 ]; then
  echo "check-wasm-fixtures: $failed of $checked fixture(s) differ from their source" >&2
  exit 1
fi

echo "check-wasm-fixtures: $checked fixture(s) checked, all match their source"
