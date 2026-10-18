#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Shared file: the same copy lives in ujira, ujira-contracts, ujira-docs and
# ujira-mcp (qomiralabs/ujira#303). Change all four together.
#
# Self test for check-dashes.sh. Run: scripts/test-check-dashes.sh
# Each case builds a throwaway git repository, commits files to it and checks
# the exit code and output. Dashes are written as bytes, so this file holds
# none.
set -euo pipefail

script="$(cd "$(dirname "$0")" && pwd)/check-dashes.sh"
work=$(cd "$(mktemp -d)" && pwd -P)
trap 'rm -rf "$work"' EXIT

passed=0
failures=0
em=$(printf '\xe2\x80\x94')
en=$(printf '\xe2\x80\x93')

# repo <name>: a fresh repository; files are added by the caller.
repo() {
  dir="$work/$1"
  git init -q "$dir"
  git -C "$dir" config user.name Test
  git -C "$dir" config user.email test@example.invalid
  git -C "$dir" config commit.gpgsign false
  git -C "$dir" config core.hooksPath /dev/null
}

commit() { git -C "$1" add -A && git -C "$1" commit -q --allow-empty -m "test"; }

# expect <case name> <wanted exit code> <dir> [text the output must contain]
expect() {
  local name=$1 want=$2 dir=$3 needle=${4:-} got=0 out
  out=$(cd "$dir" && "$script" 2>&1) || got=$?
  if [ "$got" -eq "$want" ] && { [ -z "$needle" ] || grep -q -F -e "$needle" <<<"$out"; }; then
    echo "PASS  $name"
    passed=$((passed + 1))
  else
    echo "FAIL  $name (wanted exit $want${needle:+ and output $needle}, got $got)"
    printf '%s\n' "$out" | sed 's/^/      /'
    failures=$((failures + 1))
  fi
}

repo clean
printf 'plain text, no dashes - only a hyphen\n' >"$work/clean/page.md"
commit "$work/clean"
expect "a clean file passes" 0 "$work/clean" "No em or en dashes found."

repo em
printf 'one\nbad %s here\n' "$em" >"$work/em/page.md"
commit "$work/em"
expect "an em dash fails with its path and line" 1 "$work/em" "page.md:2:"

repo en
printf 'pages 1%s5\n' "$en" >"$work/en/page.md"
commit "$work/en"
expect "an en dash fails with its path and line" 1 "$work/en" "page.md:1:"

repo binary
printf '\000\001%s\002' "$em" >"$work/binary/image.bin"
commit "$work/binary"
expect "a binary file is skipped" 0 "$work/binary"

repo space
mkdir -p "$work/space/my docs"
printf 'bad %s here\n' "$em" >"$work/space/my docs/a page.md"
commit "$work/space"
expect "a file whose name has a space is checked" 1 "$work/space" "my docs/a page.md:1:"

repo nested
mkdir -p "$work/nested/sub"
printf 'clean\n' >"$work/nested/top.md"
printf 'x %s y\n' "$en" >"$work/nested/sub/deep.ts"
commit "$work/nested"
(cd "$work/nested/sub" && "$script" >/dev/null 2>&1) && got=0 || got=$?
if [ "$got" -eq 1 ]; then
  echo "PASS  run from a subfolder still checks the whole repository"
  passed=$((passed + 1))
else
  echo "FAIL  run from a subfolder still checks the whole repository (got $got)"
  failures=$((failures + 1))
fi

repo untracked
printf 'clean\n' >"$work/untracked/page.md"
commit "$work/untracked"
printf 'bad %s here\n' "$em" >"$work/untracked/scratch.md"
expect "an untracked file is not checked" 0 "$work/untracked"

mkdir -p "$work/not-a-repo"
expect "outside a git repository it fails closed" 2 "$work/not-a-repo"

# A git that finds the repository but cannot search it.
mkdir -p "$work/fake-bin"
cat >"$work/fake-bin/git" <<FAKE
#!/usr/bin/env bash
if [ "\$1" = "rev-parse" ]; then echo "$work/clean"; exit 0; fi
exit 128
FAKE
chmod +x "$work/fake-bin/git"
got=0
(cd "$work/clean" && PATH="$work/fake-bin:$PATH" "$script" >/dev/null 2>&1) || got=$?
if [ "$got" -eq 2 ]; then
  echo "PASS  a git failure fails closed"
  passed=$((passed + 1))
else
  echo "FAIL  a git failure fails closed (got $got)"
  failures=$((failures + 1))
fi

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
