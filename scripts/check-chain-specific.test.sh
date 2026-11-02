#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Self test for check-chain-specific.sh. Each case builds a throwaway git repo
# and checks the exit code and that the output names the hit, so a case cannot
# pass by failing for some other reason. The words are assembled at run time, so
# this file does not hit the check it tests.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
script="$here/check-chain-specific.sh"
work=$(cd "$(mktemp -d)" && pwd -P)
trap 'rm -rf "$work"' EXIT

passed=0
failures=0

ce='ce''lo'
Ce='Ce''lo'
fo='for''ge'
fy='for''gery'
fd='foun''dry'
so='.so''l'
sl='sli''ther'
sd='soli''dity'
ev='EV''M'
ol='Once''Lock'
cu='CE''LO'

new_repo() {
  repo="$work/$1"
  mkdir -p "$repo/scripts"
  git -C "$repo" init -q
}

# write <path> <content>, then stage it
write() {
  mkdir -p "$(dirname "$repo/$1")"
  printf '%s\n' "$2" >"$repo/$1"
  git -C "$repo" add -f -- "$1"
}

# allow <line>...: the allowlist, staged
allow() {
  printf '%s\n' "$@" >"$repo/scripts/chain-specific-allowlist.txt"
  git -C "$repo" add -f -- scripts/chain-specific-allowlist.txt
}

expect() {
  local name=$1 want=$2 out code=0 needle
  shift 2
  out=$(cd "$repo" && "$script" 2>&1) || code=$?
  if [ "$code" -ne "$want" ]; then
    echo "FAIL: $name: exit $code, want $want"
    printf '%s\n' "$out" | sed 's/^/    /'
    failures=$((failures + 1))
    return
  fi
  for needle in "$@"; do
    if [[ "$out" != *"$needle"* ]]; then
      echo "FAIL: $name: output does not mention '$needle'"
      printf '%s\n' "$out" | sed 's/^/    /'
      failures=$((failures + 1))
      return
    fi
  done
  echo "ok: $name"
  passed=$((passed + 1))
}

new_repo clean
write README.md "Runs on Stellar."
expect "a clean tree passes" 0 "no unlisted hits, 0 allowlist entries"

new_repo sol_file
write "contracts/Token$so" "contract Token {}"
expect "a new $so file fails" 1 "contracts/Token$so: file name: Token$so"

new_repo sol_empty
: >"$repo/Token$so"
git -C "$repo" add -f -- "Token$so"
expect "an empty $so file fails on its name" 1 "Token$so: file name: Token$so"

new_repo readme
write README.md "Also runs on $Ce today."
expect "a $ce mention in a README fails" 1 "README.md:1: $Ce"

new_repo workflow
write .github/workflows/ci.yml "      - run: $fo test"
expect "a $fo command in a workflow fails" 1 ".github/workflows/ci.yml:1: $fo"

for word in "$fd" "$fo" "$sl" "$sd" "$ev"; do
  new_repo "word_$word"
  write src/a.rs "// mentions $word here"
  expect "the word $word fails" 1 "src/a.rs:1: $word"
done

new_repo upper
write src/a.rs "// $(printf '%s' "$ce" | tr '[:lower:]' '[:upper:]')"
expect "matching ignores case" 1 "src/a.rs:1: $cu"

new_repo inside_word
write src/a.rs "let x = $ol::new();"
expect "a hit inside a longer word is reported whole" 1 "src/a.rs:1: $ol"

new_repo listed
write src/test.rs "// a $fy attempt"
allow "# The word $fy, not a build tool." "src/test.rs:$fy"
expect "an allowlisted hit passes" 0 "1 allowlist entries all in use"

new_repo listed_name
write "fixtures/x-$ce-only.json" "{}"
allow "# Named for a removed chain in a checker fixture." "fixtures/x-$ce-only.json:x-$ce-only.json"
expect "an allowlisted file name passes" 0 "1 allowlist entries all in use"

new_repo second_hit
write src/test.rs "// a $fy attempt
// also $ce"
allow "# The word $fy, not a build tool." "src/test.rs:$fy"
expect "an allowlisted path with a second, unlisted hit fails" 1 "src/test.rs:2: $ce"

new_repo other_file
write src/test.rs "// a $fy attempt"
write src/other.rs "// a $fy attempt"
allow "# The word $fy, not a build tool." "src/test.rs:$fy"
expect "an entry covers its own file only" 1 "src/other.rs:1: $fy"

new_repo exact_case
write src/test.rs "// $Ce"
allow "# Exact." "src/test.rs:$ce"
expect "an entry is compared with its case" 1 "src/test.rs:1: $Ce"

new_repo spec_mention
write spec/MANDATE.md "A $ce mention, kept on purpose."
write "spec/vectors/x$so" "text"
expect "a $ce mention under spec/ passes" 0

new_repo spec_like
write specs/notes.md "A $ce mention."
expect "a folder merely named like spec is checked" 1 "specs/notes.md:1: $ce"

new_repo exclusion_dir
write soroban/a.rs "// $ce"
allow "# Everything in here." "soroban/:$ce"
expect "an exclusion for another directory is refused" 2 "'soroban/' is not a tracked file"

new_repo exclusion_dir_bare
write soroban/a.rs "// $ce"
allow "# Everything in here." "soroban:$ce"
expect "an exclusion written without the slash is refused" 2 "'soroban' is not a tracked file"

new_repo exclusion_glob
write soroban/a.rs "// $ce"
allow "# Everything in here." "soroban/*:$ce"
expect "a wildcard exclusion is refused" 2 "'soroban/*' is not a tracked file"

new_repo no_comment
write src/test.rs "// a $fy attempt"
allow "src/test.rs:$fy"
expect "an entry without a comment is refused" 2 "has no comment saying why"

new_repo comment_block
write src/test.rs "// a $fy attempt"
write src/two.rs "// a $fy attempt"
allow "# A reason." "src/test.rs:$fy" "" "src/two.rs:$fy"
expect "a blank line ends a comment's block" 2 "'src/two.rs:$fy' has no comment saying why"

new_repo malformed
write src/test.rs "// fine"
allow "# A reason." "src/test.rs"
expect "an entry without a word is refused" 2 "is not <path>:<word>"

new_repo duplicate
write src/test.rs "// a $fy attempt"
allow "# A reason." "src/test.rs:$fy" "src/test.rs:$fy"
expect "an entry listed twice is refused" 2 "listed twice"

new_repo stale
write src/test.rs "// fine"
allow "# A reason." "src/test.rs:$fy"
expect "an entry that matches nothing fails" 1 "src/test.rs:$fy matches nothing"

new_repo blob_not_tree
write src/a.rs "// $ce"
printf 'clean now\n' >"$repo/src/a.rs"
expect "the staged content is checked, not the working tree" 1 "src/a.rs:1: $ce"

new_repo binary
printf '\000asm%s\000' "$ce" >"$repo/a.wasm"
git -C "$repo" add -f -- a.wasm
expect "binary files are not searched" 0

new_repo env_other
write a.rs "fine"
other=$repo
new_repo env_real
write a.rs "// $ce"
GIT_DIR="$other/.git" GIT_INDEX_FILE="$other/.git/index" expect "inherited git variables are ignored" 1 "a.rs:1: $ce"

mkdir -p "$work/not_a_repo"
repo="$work/not_a_repo"
GIT_CEILING_DIRECTORIES=$work expect "outside a repository exits 2" 2 "could not find the git repository" || true

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
