#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Builds the OpenZeppelin smart account from a pinned stellar-contracts tag, in
# a fresh checkout, and prints the SHA-256 of its wasm. That hash is the code a
# wallet runs on chain, so anyone can rebuild it from upstream source and
# compare, with nothing taken from this repository except the tool versions.
#
# The tag is checked against the commit pinned here, because a tag can be moved
# and a moved tag would otherwise change the code without changing the name.
# The Rust toolchain is the one soroban/rust-toolchain.toml pins, not the
# moving "stable" of the upstream repository, and stellar-cli must be the
# version CI pins, since both are recorded in the wasm's own metadata.
#
# Stdout carries only the result, so a caller can capture it. With --all it
# also prints the WebAuthn verifier and the weighted threshold policy, the
# other two contracts a passkey wallet depends on, one "name hash" per line.
# With --out-dir it also leaves the built wasm there.
#
# The checkout and its whole target directory go under TMPDIR, so point TMPDIR
# at a disk where /tmp is backed by memory. Cargo's own limits, such as
# CARGO_BUILD_JOBS, apply as usual and do not change the result.
#
# OZ_REPO overrides the upstream URL and STELLAR the stellar binary, both only
# for the self test.
#
# Usage: scripts/build-smart-account.sh [--all] [--out-dir DIR] <tag>
# Exit codes: 0 built, 1 the tag or a built hash does not match its pin or a
# tool has the wrong version, 2 bad usage, an unknown tag, a missing tool or a
# failed build.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
repo=${OZ_REPO:-https://github.com/OpenZeppelin/stellar-contracts.git}
stellar=${STELLAR:-stellar}

usage() {
  echo "usage: scripts/build-smart-account.sh [--all] [--out-dir DIR] <tag>" >&2
  exit 2
}

all=0
out_dir=""
while [ "$#" -gt 1 ]; do
  case "$1" in
    --all)
      all=1
      shift
      ;;
    --out-dir)
      [ "$#" -ge 3 ] || usage
      out_dir=$2
      shift 2
      ;;
    *) usage ;;
  esac
done
[ "$#" -eq 1 ] || usage
tag=$1

# One entry for every tag this repository has reviewed. Adding a tag here is a
# decision, backed by the audit coverage record in spikes/. A wasm hash is
# pinned only once two separate CI runs built it and agreed; until then it is
# empty and the build only reports what it measured.
case "$tag" in
  v0.8.0)
    commit=ddc327eb92d63709436da3a8455f641c2d5476c5
    pin_account=""
    pin_verifier=""
    pin_policy=""
    ;;
  *)
    echo "build-smart-account: $tag is not a pinned tag" >&2
    exit 2
    ;;
esac

remote=$(git ls-remote "$repo" "refs/tags/$tag" "refs/tags/$tag^{}") || {
  echo "build-smart-account: cannot read the tags of $repo" >&2
  exit 2
}
# An annotated tag lists the peeled commit last, a lightweight tag only itself.
found=$(printf '%s\n' "$remote" | awk 'END { print $1 }')
if [ "$found" != "$commit" ]; then
  echo "build-smart-account: $tag is ${found:-missing} upstream, but is pinned to $commit" >&2
  exit 1
fi

toolchain=$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$root/soroban/rust-toolchain.toml")
if [ -z "$toolchain" ]; then
  echo "build-smart-account: no channel in soroban/rust-toolchain.toml" >&2
  exit 2
fi
export RUSTUP_TOOLCHAIN=$toolchain

cli_pin=$(sed -n 's/^ *STELLAR_CLI_VERSION: *//p' "$root/.github/workflows/ci.yml")
if ! command -v "$stellar" >/dev/null 2>&1; then
  echo "build-smart-account: stellar is not installed" >&2
  exit 2
fi
if ! "$stellar" --version | head -n 1 | grep -q "^stellar ${cli_pin} "; then
  echo "build-smart-account: stellar is not the pinned ${cli_pin}" >&2
  exit 1
fi

# Anything the environment could add to the compile would change the hash.
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_TARGET_DIR CARGO_BUILD_TARGET_DIR
export CARGO_INCREMENTAL=0

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

git -C "$work" init -q src
git -C "$work/src" fetch -q --depth 1 "$repo" "$commit" || {
  echo "build-smart-account: cannot fetch $commit" >&2
  exit 2
}
git -C "$work/src" -c advice.detachedHead=false checkout -q "$commit"

build() {
  local package=$1 file=$2 out="$work/out/$1"
  mkdir -p "$out"
  if ! (cd "$work/src" && "$stellar" contract build --locked --package "$package" --out-dir "$out") >"$work/build.log" 2>&1; then
    sed 's/^/    /' "$work/build.log" >&2
    echo "build-smart-account: building $package failed" >&2
    exit 2
  fi
  if [ ! -f "$out/$file" ]; then
    echo "build-smart-account: building $package did not produce $file" >&2
    exit 2
  fi
  if [ -n "$out_dir" ]; then
    mkdir -p "$out_dir"
    cp "$out/$file" "$out_dir/$file"
  fi
  sha256sum "$out/$file" | cut -d' ' -f1
}

# A pinned hash that differs fails the run, after printing what was built, so
# a log always shows the hash that was measured.
mismatch=0
check() {
  local package=$1 got=$2 want=$3
  if [ -n "$want" ] && [ "$got" != "$want" ]; then
    echo "build-smart-account: $package built to $got, but $tag pins $want" >&2
    mismatch=1
  fi
}

# Each hash is taken into a variable first: a failure inside a command
# substitution that is only an argument to echo would not stop the script.
account=$(build multisig-account-example multisig_account_example.wasm)
check multisig-account-example "$account" "$pin_account"
if [ "$all" -eq 0 ]; then
  echo "$account"
  exit "$mismatch"
fi
verifier=$(build multisig-webauthn-verifier-example multisig_webauthn_verifier_example.wasm)
check multisig-webauthn-verifier-example "$verifier" "$pin_verifier"
policy=$(build multisig-weighted-threshold-policy-example multisig_weighted_threshold_policy_example.wasm)
check multisig-weighted-threshold-policy-example "$policy" "$pin_policy"
echo "multisig-account-example $account"
echo "multisig-webauthn-verifier-example $verifier"
echo "multisig-weighted-threshold-policy-example $policy"
exit "$mismatch"
