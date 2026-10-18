#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Checks that every tracked Rust source file starts with the Apache 2.0 header:
#
#   // SPDX-License-Identifier: Apache-2.0
#   // Copyright <year> Qomira Labs
#
# Both lines must be the first two lines of the file, so a license scanner
# finds the SPDX line without parsing anything.
#
# Skipped: Cargo build output (target/, soroban/target/).
#
# The file list comes from git (tracked files only), but the contents are read
# from the working tree, not the index. A file staged without a header and then
# fixed on disk passes, so this is not a substitute for checking staged content
# in a pre-commit hook.
#
# Usage: scripts/check-headers.sh [path inside the repo]
# Exit codes: 0 all headers present, 1 one or more files named on stderr,
# 2 the tree could not be listed.
set -euo pipefail

# An inherited repo override would make git -C look at some other repo.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE

root=${1:-"$(cd "$(dirname "$0")/.." && pwd)"}

if ! root=$(git -C "$root" rev-parse --show-toplevel 2>/dev/null); then
  echo "check-headers: ${1:-$root} is not inside a git work tree" >&2
  exit 2
fi

list=$(mktemp)
trap 'rm -f "$list"' EXIT

# Listing into a file, not a process substitution, so a git failure stops the
# check instead of looking like an empty tree.
if ! git -C "$root" ls-files -z -- '*.rs' >"$list"; then
  echo "check-headers: git ls-files failed in $root" >&2
  exit 2
fi

spdx_line='// SPDX-License-Identifier: Apache-2.0'
copyright_re='^// Copyright [0-9]{4} Qomira Labs$'

skipped() {
  case "$1" in
    target/* | soroban/target/*) return 0 ;;
    *) return 1 ;;
  esac
}

checked=0
failed=0
while IFS= read -r -d '' file; do
  skipped "$file" && continue
  path="$root/$file"

  if [ -L "$path" ] && [ ! -e "$path" ]; then
    checked=$((checked + 1))
    echo "missing header: $file (dangling symlink)" >&2
    failed=$((failed + 1))
    continue
  fi
  # A tracked file deleted in the working tree has nothing to check.
  [ -f "$path" ] || continue
  checked=$((checked + 1))

  line1=$(sed -n '1p' "$path" | tr -d '\r')
  line2=$(sed -n '2p' "$path" | tr -d '\r')

  if [ "$line1" != "$spdx_line" ]; then
    echo "missing header: $file (line 1 must be '$spdx_line')" >&2
    failed=$((failed + 1))
  elif ! printf '%s\n' "$line2" | grep -Eq "$copyright_re"; then
    echo "missing header: $file (line 2 must be '// Copyright <year> Qomira Labs')" >&2
    failed=$((failed + 1))
  fi
done <"$list"

if [ "$failed" -gt 0 ]; then
  echo "check-headers: $failed of $checked file(s) missing the Apache 2.0 header" >&2
  exit 1
fi

echo "check-headers: $checked file(s) checked, all have the Apache 2.0 header"
