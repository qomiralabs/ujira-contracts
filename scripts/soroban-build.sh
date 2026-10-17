#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Builds the production Soroban contracts, and only those, with
# `stellar contract build`.
#
# A production contract is a workspace crate under soroban/contracts/ with a
# cdylib target. Spike and test crates live elsewhere in the workspace, and a
# plain `stellar contract build` would build them too, next to the production
# wasm. Each production crate is built on its own with --package instead, and
# the release folder must then hold exactly one wasm per production crate.
# Any wasm already there, for example one restored from a CI cache, is removed
# before the build.
#
# Needs cargo, jq and stellar (stellar only when building).
#
# Usage: scripts/soroban-build.sh [--list] [soroban dir]
#   --list  print the production package names and stop
# Exit codes: 0 built, 1 a build failed or the release folder is wrong,
# 2 the workspace could not be read or has no production contract.
set -euo pipefail

list_only=0
if [ "${1:-}" = "--list" ]; then
  list_only=1
  shift
fi
ws=${1:-"$(cd "$(dirname "$0")/.." && pwd)/soroban"}

if [ ! -f "$ws/Cargo.toml" ]; then
  echo "soroban-build: no Cargo.toml in $ws" >&2
  exit 2
fi
for tool in cargo jq; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "soroban-build: $tool is not installed" >&2
    exit 2
  fi
done

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# Run from the workspace so its rust-toolchain.toml applies.
if ! (cd "$ws" && cargo metadata --locked --offline --no-deps --format-version 1) >"$tmp/meta.json" 2>"$tmp/meta.err"; then
  cat "$tmp/meta.err" >&2
  echo "soroban-build: cargo metadata failed in $ws" >&2
  exit 2
fi

# <package name> TAB <wasm file name>, one line per production contract. The
# wasm is named after the lib target, with dashes turned into underscores.
jq -r '(.workspace_root + "/contracts/") as $dir
  | .packages[]
  | select(.manifest_path | startswith($dir))
  | . as $pkg
  | ([.targets[] | select(.crate_types | index("cdylib"))] | first) // empty
  | [$pkg.name, ((.name | gsub("-"; "_")) + ".wasm")]
  | @tsv' "$tmp/meta.json" | sort >"$tmp/contracts.tsv"

if [ ! -s "$tmp/contracts.tsv" ]; then
  echo "soroban-build: no production contracts under $ws/contracts" >&2
  exit 2
fi

if [ "$list_only" -eq 1 ]; then
  cut -f1 "$tmp/contracts.tsv"
  exit 0
fi

if ! command -v stellar >/dev/null 2>&1; then
  echo "soroban-build: stellar is not installed" >&2
  exit 2
fi

target=$(jq -r '.target_directory' "$tmp/meta.json")
release="$target/wasm32v1-none/release"
if [ -d "$release" ]; then
  find "$release" -maxdepth 1 -name '*.wasm' -type f -delete
fi

while IFS=$'\t' read -r package _; do
  echo "soroban-build: building $package"
  if ! (cd "$ws" && stellar contract build --locked --package "$package"); then
    echo "soroban-build: stellar contract build failed for $package" >&2
    exit 1
  fi
done <"$tmp/contracts.tsv"

cut -f2 "$tmp/contracts.tsv" | sort >"$tmp/want"
if [ -d "$release" ]; then
  find "$release" -maxdepth 1 -name '*.wasm' -exec basename {} \; | sort >"$tmp/have"
else
  : >"$tmp/have"
fi

bad=0
while IFS= read -r wasm; do
  echo "soroban-build: unexpected wasm in the release folder: $wasm" >&2
  bad=1
done < <(comm -13 "$tmp/want" "$tmp/have")
while IFS= read -r wasm; do
  echo "soroban-build: missing wasm in the release folder: $wasm" >&2
  bad=1
done < <(comm -23 "$tmp/want" "$tmp/have")
if [ "$bad" -ne 0 ]; then
  exit 1
fi

echo "soroban-build: release folder holds exactly: $(tr '\n' ' ' <"$tmp/want" | sed 's/ $//')"
