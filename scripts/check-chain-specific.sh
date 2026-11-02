#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# UJIRA runs on Stellar only. Fails when a tracked file, or a tracked path,
# holds a word from another chain's stack, so removed code cannot come back
# unnoticed. The spec under spec/ is left out because it keeps its chain-neutral
# rules on purpose, and nothing else is left out except the allowlist itself,
# whose entries have to name these words.
#
# The words are matched case insensitively, and a hit is the whole word around
# the match. Index content is read, not the working tree. Files git treats as
# binary are not searched, which a NUL byte cannot abuse because
# check-invisible-characters.py refuses a NUL in anything but a real binary.
#
# scripts/chain-specific-allowlist.txt holds exact <path>:<word> entries, one per
# line. The path must be a tracked file, never a directory or a wildcard, and the
# word is compared exactly, case included. Every entry needs a comment above it
# in its block, saying why, and an entry that no longer matches anything fails,
# so the list only holds what is really there. An entry allows that word in that
# file; a different word in the same file still fails.
#
# The word list below is assembled from fragments so this file does not hit it.
#
# Usage: scripts/check-chain-specific.sh
# Exit codes: 0 clean, 1 hits outside the allowlist or stale entries, 2 a bad
# allowlist entry, or git could not search the repository.
set -euo pipefail

# Any GIT_ variable could point git at another repository, index or config, so
# none is inherited.
while IFS= read -r name; do
  [ -n "$name" ] || continue
  unset "$name"
done <<<"$(env | sed -n 's/^\(GIT_[A-Za-z0-9_]*\)=.*/\1/p')"

if ! top=$(git rev-parse --show-toplevel 2>/dev/null) || [ -z "$top" ]; then
  echo "check-chain-specific: could not find the git repository" >&2
  exit 2
fi

words='ce''lo|foun''dry|for''ge|sli''ther|soli''dity|\.so''l\b|ev''m'
hit_pattern="[[:alnum:]_.-]*($words)[[:alnum:]_.-]*"
allowlist_name=scripts/chain-specific-allowlist.txt
allowlist="$top/$allowlist_name"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

if ! git -C "$top" -c core.quotePath=false ls-files -z >"$work/tracked"; then
  echo "check-chain-specific: git could not list the tracked files" >&2
  exit 2
fi
declare -A tracked=()
while IFS= read -r -d '' path; do
  tracked[$path]=1
done <"$work/tracked"

# Reads the allowlist. A comment line opens a block and a blank line closes it,
# and an entry has to sit in a block that has a comment.
declare -A allowed=()
declare -A allowed_at=()
bad=0
refuse() {
  echo "check-chain-specific: $allowlist_name:$1: $2" >&2
  bad=1
}
if [ -f "$allowlist" ]; then
  number=0
  commented=0
  while IFS= read -r line || [ -n "$line" ]; do
    number=$((number + 1))
    case $line in
      '')
        commented=0
        continue
        ;;
      '#'*)
        commented=1
        continue
        ;;
    esac
    case $line in
      *:*) ;;
      *)
        refuse "$number" "'$line' is not <path>:<word>"
        continue
        ;;
    esac
    path=${line%:*}
    word=${line##*:}
    if [ -z "$path" ] || [ -z "$word" ]; then
      refuse "$number" "'$line' is not <path>:<word>"
    elif [ "$commented" -ne 1 ]; then
      refuse "$number" "'$line' has no comment saying why"
    elif [ -z "${tracked[$path]:-}" ]; then
      refuse "$number" "'$path' is not a tracked file, and only exact files may be listed, never a directory or a wildcard"
    elif [ -n "${allowed[$line]:-}" ]; then
      refuse "$number" "'$line' is listed twice"
    else
      allowed[$line]=0
      allowed_at[$line]=$number
    fi
  done <"$allowlist"
fi
if [ "$bad" -ne 0 ]; then
  exit 2
fi

found=0

# Content, one hit per line: path NUL line NUL word.
code=0
LC_ALL=C git -C "$top" -c core.quotePath=false grep --cached -I -z -n -i -o -E "$hit_pattern" \
  -- . ':!spec/' ":!$allowlist_name" >"$work/hits" || code=$?
if [ "$code" -gt 1 ]; then
  echo "check-chain-specific: git could not search the repository (exit $code)" >&2
  exit 2
fi
while IFS= read -r -d '' path && IFS= read -r -d '' number && IFS= read -r word; do
  key="$path:$word"
  if [ -n "${allowed[$key]:-}" ]; then
    allowed[$key]=1
  else
    printf '%s:%s: %s\n' "$path" "$number" "$word"
    found=1
  fi
done <"$work/hits"

# Paths, so a file holding nothing searchable, such as an empty one, still
# counts. Each part of the path is one word.
while IFS= read -r -d '' path; do
  case $path in
    spec/* | "$allowlist_name") continue ;;
  esac
  while IFS= read -r word; do
    [ -n "$word" ] || continue
    key="$path:$word"
    if [ -n "${allowed[$key]:-}" ]; then
      allowed[$key]=1
    else
      printf '%s: file name: %s\n' "$path" "$word"
      found=1
    fi
  done <<<"$(printf '%s' "$path" | LC_ALL=C grep -i -o -E "$hit_pattern" || true)"
done <"$work/tracked"

for key in "${!allowed[@]}"; do
  if [ "${allowed[$key]}" -eq 0 ]; then
    printf '%s:%s: %s matches nothing, remove the entry\n' "$allowlist_name" "${allowed_at[$key]}" "$key"
    found=1
  fi
done

if [ "$found" -ne 0 ]; then
  echo "check-chain-specific: chain-specific words found. Remove them, or add an exact entry with a reason to $allowlist_name." >&2
  exit 1
fi
echo "check-chain-specific: no unlisted hits, ${#allowed[@]} allowlist entries all in use"
