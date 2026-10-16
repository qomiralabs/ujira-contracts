#!/usr/bin/env bash
# Self test for check-headers.sh. Each case builds a throwaway git repo in a
# temp directory, stages some files and checks the exit code and output.
set -euo pipefail

script="$(cd "$(dirname "$0")" && pwd)/check-headers.sh"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

header='// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs'

passed=0
failures=0

new_repo() {
  repo="$work/$1"
  mkdir -p "$repo"
  git -C "$repo" init -q
}

# write <path> <content>, then stage it
write() {
  mkdir -p "$(dirname "$repo/$1")"
  printf '%s\n' "$2" >"$repo/$1"
  git -C "$repo" add -f -- "$1"
}

# expect <case name> <exit code> [text the output must contain]
expect() {
  local name=$1 want=$2 needle=${3:-}
  local out code=0
  out=$("$script" "$repo" 2>&1) || code=$?
  if [ "$code" -ne "$want" ]; then
    echo "FAIL: $name: exit $code, want $want"
    printf '%s\n' "$out" | sed 's/^/    /'
    failures=$((failures + 1))
    return
  fi
  if [ -n "$needle" ] && ! printf '%s\n' "$out" | grep -qF -- "$needle"; then
    echo "FAIL: $name: output does not mention '$needle'"
    printf '%s\n' "$out" | sed 's/^/    /'
    failures=$((failures + 1))
    return
  fi
  echo "ok: $name"
  passed=$((passed + 1))
}

new_repo empty
expect "empty tree passes" 0

new_repo good
write soroban/src/lib.rs "$header
#![no_std]"
write celo/src/Mandate.sol "$header
pragma solidity 0.8.28;"
expect "files with header pass" 0 "2 file(s) checked"

new_repo rust_missing
write soroban/src/ok.rs "$header"
write soroban/src/lib.rs '#![no_std]'
expect "rust file without header" 1 "soroban/src/lib.rs"

new_repo sol_missing
write celo/src/Mandate.sol 'pragma solidity 0.8.28;'
expect "solidity file without header" 1 "celo/src/Mandate.sol"

new_repo sol_spdx_not_first
write celo/src/Mandate.sol "pragma solidity 0.8.28;
$header"
expect "solidity header not on first line" 1 "celo/src/Mandate.sol"

new_repo copyright_missing
write soroban/src/lib.rs '// SPDX-License-Identifier: Apache-2.0
#![no_std]'
expect "copyright line missing" 1 "soroban/src/lib.rs"

new_repo wrong_license
write soroban/src/lib.rs '// SPDX-License-Identifier: MIT
// Copyright 2026 Qomira Labs'
expect "wrong license identifier" 1 "soroban/src/lib.rs"

new_repo crlf
write soroban/src/lib.rs "$(printf '// SPDX-License-Identifier: Apache-2.0\r\n// Copyright 2026 Qomira Labs\r')"
expect "header with CRLF line endings passes" 0

new_repo vendored
write celo/lib/forge-std/src/Test.sol 'pragma solidity ^0.8.0;'
write celo/src/Mandate.sol "$header"
expect "vendored lib ignored" 0 "1 file(s) checked"

new_repo build_output
write soroban/target/debug/build/gen.rs 'fn main() {}'
write celo/out/Gen.sol 'pragma solidity ^0.8.0;'
write celo/cache/Gen.sol 'pragma solidity ^0.8.0;'
expect "build output ignored" 0 "0 file(s) checked"

new_repo untracked
write soroban/src/lib.rs "$header"
printf 'fn scratch() {}\n' >"$repo/soroban/src/scratch.rs"
expect "untracked file ignored" 0 "1 file(s) checked"

new_repo reports_all
write a.rs 'fn a() {}'
write b.sol 'contract B {}'
expect "every offending file is named" 1 "2 of 2 file(s)"

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
