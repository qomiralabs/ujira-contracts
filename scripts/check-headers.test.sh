#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Self test for check-headers.sh. Each case builds a throwaway git repo in a
# temp directory, stages some files and checks the exit code and output.
set -euo pipefail

script="$(cd "$(dirname "$0")" && pwd)/check-headers.sh"
work=$(cd "$(mktemp -d)" && pwd -P)
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

# expect <case name> <exit code> [text the output must contain]...
# Runs the check against $target if set, otherwise against $repo.
expect() {
  local name=$1 want=$2
  shift 2
  local out code=0 needle
  out=$("$script" "${target:-$repo}" 2>&1) || code=$?
  if [ "$code" -ne "$want" ]; then
    echo "FAIL: $name: exit $code, want $want"
    printf '%s\n' "$out" | sed 's/^/    /'
    failures=$((failures + 1))
    return
  fi
  for needle in "$@"; do
    if ! printf '%s\n' "$out" | grep -qF -- "$needle"; then
      echo "FAIL: $name: output does not mention '$needle'"
      printf '%s\n' "$out" | sed 's/^/    /'
      failures=$((failures + 1))
      return
    fi
  done
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
expect "every offending file is named" 1 "a.rs" "b.sol" "2 of 2 file(s)"

new_repo root_target
write target/debug/build/gen.rs 'fn main() {}'
expect "root target output ignored" 0 "0 file(s) checked"

new_repo nested_target
write soroban/src/target/mod.rs 'pub mod x;'
expect "source folder named target is checked" 1 "soroban/src/target/mod.rs"

new_repo space_name
write "soroban/src/my file.rs" 'fn a() {}'
expect "filename with a space is named" 1 "soroban/src/my file.rs"

new_repo dangling
mkdir -p "$repo/soroban/src"
ln -s missing.rs "$repo/soroban/src/link.rs"
git -C "$repo" add -- soroban/src/link.rs
expect "dangling symlink is reported" 1 "soroban/src/link.rs (dangling symlink)"

new_repo subfolder_root
write celo/lib/forge-std/src/Test.sol 'pragma solidity ^0.8.0;'
write celo/src/Mandate.sol "$header"
target="$repo/celo"
expect "subfolder root resolves to the top level" 0 "1 file(s) checked"
unset target

mkdir -p "$work/not_a_repo"
target="$work/not_a_repo"
GIT_CEILING_DIRECTORIES=$work expect "root outside a git repo exits 2" 2 "not inside a git work tree"
unset target

new_repo other_repo
write soroban/src/lib.rs "$header"
other="$repo"
new_repo git_dir_override
write soroban/src/lib.rs 'fn a() {}'
GIT_DIR="$other/.git" GIT_WORK_TREE="$other" GIT_INDEX_FILE="$other/.git/index" \
  expect "inherited GIT_DIR does not redirect the check" 1 "soroban/src/lib.rs" "1 of 1 file(s)"

new_repo git_fails
write soroban/src/lib.rs 'fn a() {}'
real_git=$(command -v git)
mkdir -p "$work/fakebin"
cat >"$work/fakebin/git" <<FAKE
#!/usr/bin/env bash
for arg in "\$@"; do
  if [ "\$arg" = ls-files ]; then echo "fatal: index file corrupt" >&2; exit 128; fi
done
exec "$real_git" "\$@"
FAKE
chmod +x "$work/fakebin/git"
PATH="$work/fakebin:$PATH" expect "git ls-files failure exits 2" 2 "git ls-files failed"

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
