#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Scans the full history of HEAD for secrets with gitleaks and this repo's
# .gitleaks.toml. Fails on any finding. Also fails unless gitleaks reports
# scanning every commit that changes a file (merge commits have no diff of
# their own), so a shallow clone or a narrowed range cannot pass as clean.
#
# Inline gitleaks:allow comments and .gitleaksignore files are not honoured:
# the reviewed .gitleaks.toml is the only allowlist. Findings are redacted.
#
# Usage: scripts/secret-scan.sh [path inside a git repo, default this repo]
# Exit codes: 0 clean, 1 secrets found or a .gitleaksignore present, 2 the
# scan could not run or was incomplete.
set -euo pipefail

# An inherited repo override would make git -C look at some other repo.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE

here=$(cd "$(dirname "$0")/.." && pwd)
config="$here/.gitleaks.toml"
target=${1:-$here}

if ! command -v gitleaks >/dev/null 2>&1; then
  echo "secret-scan: gitleaks is not installed" >&2
  exit 2
fi
if [ ! -f "$config" ]; then
  echo "secret-scan: $config is missing" >&2
  exit 2
fi
if ! root=$(git -C "$target" rev-parse --show-toplevel 2>/dev/null); then
  echo "secret-scan: $target is not inside a git work tree" >&2
  exit 2
fi
if [ "$(git -C "$root" rev-parse --is-shallow-repository)" != false ]; then
  echo "secret-scan: $root is a shallow clone, fetch the full history" >&2
  exit 2
fi

# gitleaks reads a .gitleaksignore at the root of the scanned tree whatever
# --gitleaks-ignore-path says, so one would silently hide findings.
ignores=$(git -C "$root" ls-files -- .gitleaksignore '*/.gitleaksignore')
if [ -n "$ignores" ] || [ -e "$root/.gitleaksignore" ]; then
  echo "secret-scan: .gitleaksignore is not allowed, add a reviewed allowlist to .gitleaks.toml instead: ${ignores:-.gitleaksignore}" >&2
  exit 1
fi

stats=$(mktemp)
log=$(mktemp)
empty=$(mktemp -d)
trap 'rm -rf "$stats" "$log" "$empty"' EXIT

if ! git -C "$root" log --no-merges --format='commit %H' --shortstat HEAD >"$stats"; then
  echo "secret-scan: git log failed in $root" >&2
  exit 2
fi
expected=$(grep -c ' changed' "$stats" || true)

# Leaks exit 3 so they can be told apart from gitleaks failing to run.
code=0
gitleaks git "$root" \
  --config "$config" \
  --log-opts HEAD \
  --gitleaks-ignore-path "$empty" \
  --ignore-gitleaks-allow \
  --redact \
  --verbose \
  --no-banner \
  --no-color \
  --exit-code 3 >"$log" 2>&1 || code=$?
cat "$log"

if [ "$code" -eq 3 ]; then
  echo "secret-scan: secrets found in the history of $root" >&2
  exit 1
fi
if [ "$code" -ne 0 ]; then
  echo "secret-scan: gitleaks failed with exit code $code" >&2
  exit 2
fi

scanned=$(sed -n 's/.* \([0-9][0-9]*\) commits scanned.*/\1/p' "$log" | head -n 1)
echo "secret-scan: ${scanned:-no} commits scanned, ${expected} commits on HEAD change files"
if [ -z "$scanned" ] || [ "$scanned" -ne "$expected" ]; then
  echo "secret-scan: gitleaks did not scan every commit on HEAD" >&2
  exit 2
fi
echo "secret-scan: no secrets found"
