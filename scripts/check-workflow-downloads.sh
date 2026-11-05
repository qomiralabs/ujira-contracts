#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Every tool a workflow downloads must survive a passing failure and prove what it
# got. A 504 from a release host used to fail a whole job and cost a rerun, so each
# curl must retry and have a time limit. And each must be followed by a checksum
# check, so that retrying can never become a way to accept whatever came back.
#
# Usage: scripts/check-workflow-downloads.sh [workflow directory]
# Exit codes: 0 every download keeps the rules, 1 one does not.
set -euo pipefail

here=$(cd "$(dirname "$0")/.." && pwd)
dir=${1:-$here/.github/workflows}
required=(--retry --retry-delay --retry-all-errors --connect-timeout --max-time)
# How many lines after the download its checksum check may come.
window=3
problems=0
downloads=0

for file in "$dir"/*.yml; do
  [ -e "$file" ] || continue
  while IFS=: read -r number line; do
    # A comment that mentions curl downloads nothing.
    case "${line#"${line%%[![:space:]]*}"}" in \#*) continue ;; esac
    downloads=$((downloads + 1))
    for flag in "${required[@]}"; do
      if ! grep -q -- "$flag " <<<"$line" && ! grep -q -- "$flag\$" <<<"$line"; then
        echo "check-workflow-downloads: ${file##*/}:$number: curl has no $flag" >&2
        problems=$((problems + 1))
      fi
    done
    if ! sed -n "$((number + 1)),$((number + window))p" "$file" |
      grep -Eq 'sha(256|512)sum --check --strict'; then
      echo "check-workflow-downloads: ${file##*/}:$number: no checksum check after the download" >&2
      problems=$((problems + 1))
    fi
  done < <(grep -nE '(^|[[:space:]])curl[[:space:]]' "$file" || true)
done

if [ "$problems" -gt 0 ]; then
  echo "check-workflow-downloads: $problems problem(s) in $downloads download(s)" >&2
  exit 1
fi
echo "check-workflow-downloads: $downloads download(s) retry and check their checksum"
