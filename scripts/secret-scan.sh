#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Scans the full history of HEAD for secrets with gitleaks and fails on any
# finding. Merge commits are scanned against each parent (-m), so a key added
# while resolving a merge is found, and every file is read as text (--text,
# --no-ext-diff, --no-textconv), so a .gitattributes entry such as -diff or
# binary cannot hide one.
#
# The rules come from this repo's .gitleaks.toml. On a pull request run, set
# SECRET_SCAN_BASE_REF to the base branch name: the rules are then read from
# origin/<base> instead, so a change cannot loosen the rules its own scan
# uses. A base without a .gitleaks.toml gets the gitleaks default rules.
#
# Inline gitleaks:allow comments are ignored and a .gitleaksignore fails the
# scan, so the reviewed .gitleaks.toml is the only allowlist. Findings are
# redacted.
#
# Key files (.pem, .key, .p12, .pfx, .jks, id_rsa and the other ssh key
# names) are refused by name in any commit, binary or not, because a binary
# key store cannot be line scanned. gitleaks still reads every file as text,
# so a key stored as plain text inside a binary is found, but a compressed or
# encoded one is not, and neither is a token split across lines.
#
# Completeness: gitleaks must report scanning at least every commit that adds
# a line of text (the only commits that can add a secret) and at most every
# commit on HEAD, and a shallow clone is refused.
#
# Usage: [SECRET_SCAN_BASE_REF=<branch>] scripts/secret-scan.sh [path inside
# a git repo, default this repo]
# Exit codes: 0 clean, 1 secrets found or a .gitleaksignore present, 2 the
# scan could not run or was incomplete.
set -euo pipefail

# An inherited repo override would make git -C look at some other repo.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE

here=$(cd "$(dirname "$0")/.." && pwd)
target=${1:-$here}
base=${SECRET_SCAN_BASE_REF:-}
# The same options for gitleaks and for the completeness count below.
log_opts=(-m --text --no-ext-diff --no-textconv HEAD)

if ! command -v gitleaks >/dev/null 2>&1; then
  echo "secret-scan: gitleaks is not installed" >&2
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

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir "$tmp/empty"

if [ -n "$base" ]; then
  if ! git -C "$root" rev-parse --verify --quiet "origin/${base}^{commit}" >/dev/null; then
    echo "secret-scan: base branch origin/$base not found" >&2
    exit 2
  fi
  config="$tmp/gitleaks.toml"
  if git -C "$root" cat-file -e "origin/${base}:.gitleaks.toml" 2>/dev/null; then
    git -C "$root" show "origin/${base}:.gitleaks.toml" >"$config"
    echo "secret-scan: rules from .gitleaks.toml on origin/$base"
  else
    printf '[extend]\nuseDefault = true\n' >"$config"
    echo "secret-scan: origin/$base has no .gitleaks.toml, using the gitleaks default rules"
  fi
  if ! git -C "$root" diff --quiet "origin/$base" HEAD -- .gitleaks.toml; then
    echo "secret-scan: this change edits .gitleaks.toml; the edit applies to scans once it is merged"
  fi
else
  config="$here/.gitleaks.toml"
  if [ ! -f "$config" ]; then
    echo "secret-scan: $config is missing" >&2
    exit 2
  fi
fi

total=$(git -C "$root" rev-list --count HEAD)
if ! git -C "$root" log -p -U0 --no-color --format='commit %H' "${log_opts[@]}" >"$tmp/log"; then
  echo "secret-scan: git log failed in $root" >&2
  exit 2
fi
# Commits with at least one added line. With -U0 every line after a hunk
# header is a change, and "+++" file headers only appear before the first
# hunk of a file.
adding=$(LC_ALL=C awk '
  /^commit [0-9a-f]+( |$)/ { sha = $2; next }
  /^diff --git / { header = 1; next }
  /^@@/ { header = 0; next }
  !header && /^\+/ { print sha }
' "$tmp/log" | LC_ALL=C sort -u | wc -l | tr -d ' ')

# Every path any commit added or changed, merges included, checked by name.
if ! git -C "$root" log -m --no-renames --name-only --diff-filter=AMCRT --format= HEAD >"$tmp/paths"; then
  echo "secret-scan: git log failed in $root" >&2
  exit 2
fi
keyfiles=$(LC_ALL=C grep -i -E '\.(pem|key|p12|pfx|jks)$|(^|/)id_(rsa|dsa|ecdsa|ed25519)[^/]*$' "$tmp/paths" | LC_ALL=C sort -u || true)

# Leaks exit 3 so they can be told apart from gitleaks failing to run.
code=0
gitleaks git "$root" \
  --config "$config" \
  --log-opts "${log_opts[*]}" \
  --gitleaks-ignore-path "$tmp/empty" \
  --ignore-gitleaks-allow \
  --redact \
  --verbose \
  --no-banner \
  --no-color \
  --exit-code 3 >"$tmp/out" 2>&1 || code=$?
cat "$tmp/out"

if [ -n "$keyfiles" ]; then
  printf '%s\n' "$keyfiles" | sed 's/^/secret-scan: key file committed: /' >&2
fi
if [ "$code" -eq 3 ]; then
  echo "secret-scan: secrets found in the history of $root" >&2
  exit 1
fi
if [ "$code" -ne 0 ]; then
  echo "secret-scan: gitleaks failed with exit code $code" >&2
  exit 2
fi

scanned=$(sed -n 's/.* \([0-9][0-9]*\) commits scanned.*/\1/p' "$tmp/out" | head -n 1)
echo "secret-scan: ${scanned:-no} commits scanned; HEAD has ${total} commits, ${adding} of them add text"
if [ -z "$scanned" ] || [ "$scanned" -lt "$adding" ] || [ "$scanned" -gt "$total" ]; then
  echo "secret-scan: gitleaks did not scan every commit that adds text" >&2
  exit 2
fi
if [ -n "$keyfiles" ]; then
  echo "secret-scan: key files are not allowed in git, keep them in the secret manager" >&2
  exit 1
fi
echo "secret-scan: no secrets found"
