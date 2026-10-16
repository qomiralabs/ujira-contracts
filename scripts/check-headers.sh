#!/usr/bin/env bash
# Checks that every tracked Rust and Solidity source file starts with the
# Apache 2.0 header:
#
#   // SPDX-License-Identifier: Apache-2.0
#   // Copyright <year> Qomira Labs
#
# Both lines must be the first two lines of the file. Solidity needs the SPDX
# line first anyway, and holding Rust to the same rule keeps one convention.
#
# Vendored Foundry dependencies (celo/lib/) and build output (target/,
# celo/out/, celo/cache/) are skipped.
#
# Usage: scripts/check-headers.sh [repo-root]
# Exits 1 and names every offending file if any header is missing.
set -euo pipefail

root=${1:-"$(cd "$(dirname "$0")/.." && pwd)"}

if ! git -C "$root" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "check-headers: $root is not a git work tree" >&2
  exit 2
fi

spdx_line='// SPDX-License-Identifier: Apache-2.0'
copyright_re='^// Copyright [0-9]{4} Qomira Labs$'

skipped() {
  case "$1" in
    celo/lib/* | celo/out/* | celo/cache/* | target/* | */target/*) return 0 ;;
    *) return 1 ;;
  esac
}

checked=0
failed=0
while IFS= read -r -d '' file; do
  skipped "$file" && continue
  # A tracked file deleted in the working tree has nothing to check.
  [ -f "$root/$file" ] || continue
  checked=$((checked + 1))

  line1=$(sed -n '1p' "$root/$file" | tr -d '\r')
  line2=$(sed -n '2p' "$root/$file" | tr -d '\r')

  if [ "$line1" != "$spdx_line" ]; then
    echo "missing header: $file (line 1 must be '$spdx_line')" >&2
    failed=$((failed + 1))
  elif ! printf '%s\n' "$line2" | grep -Eq "$copyright_re"; then
    echo "missing header: $file (line 2 must be '// Copyright <year> Qomira Labs')" >&2
    failed=$((failed + 1))
  fi
done < <(git -C "$root" ls-files -z -- '*.rs' '*.sol')

if [ "$failed" -gt 0 ]; then
  echo "check-headers: $failed of $checked file(s) missing the Apache 2.0 header" >&2
  exit 1
fi

echo "check-headers: $checked file(s) checked, all have the Apache 2.0 header"
