#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Self test for hygiene.sh. Each case builds a throwaway git repo in a temp dir
# with a base commit and a head branch, and checks the exit code and that the
# output names the violation, so a case cannot pass by failing for some other
# reason. Dashes and attribution phrases are assembled here at run time, so this
# file holds none of them literally. The fixture commits turn hooks off because
# they plant the very text the hooks refuse; this file's own commit still goes
# through them.
set -euo pipefail

script="$(cd "$(dirname "$0")" && pwd)/hygiene.sh"
work=$(cd "$(mktemp -d)" && pwd -P)
trap 'rm -rf "$work"' EXIT

passed=0
failures=0

em=$(printf '\342\200\224')
en=$(printf '\342\200\223')
trailer='Co-''Authored-By: Someone <someone@example.com>'
generated='Gener''ated with some tool'
tool='Written with Chat''GPT'
regenerated='Regener''ated with the pinned toolchain, and philan''thropic work is fine.'
assistant_dir='.clau''de'
assistant_file='CLAU''DE.md'
generated_case='gener''ated with line'

g() {
  git -C "$repo" -c core.hooksPath=/dev/null -c user.name=Test -c user.email=test@example.com "$@"
}

# new_repo <name>: a repo with one commit on main and a branch "topic" checked out.
new_repo() {
  repo="$work/$1"
  mkdir -p "$repo"
  git -C "$repo" init -q -b main
  printf 'first line\n' >"$repo/base.txt"
  g add -A
  g commit -q -m 'chore: base'
  g checkout -q -b topic
}

# commit_file <path> <content> [message]
commit_file() {
  mkdir -p "$(dirname "$repo/$1")"
  printf '%s\n' "$2" >"$repo/$1"
  g add -f -- "$1"
  g commit -q -m "${3:-chore: change}"
}

# expect <case name> <exit code> [text the output must contain]...
expect() {
  local name=$1 want=$2 out code=0 needle
  shift 2
  out=$(cd "$repo" && sh "$script" "${range_base:-main}" "${range_head:-topic}" 2>&1) || code=$?
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
commit_file a.txt "plain text" "feat: add a"
expect "clean range" 0 "hygiene: clean over main..topic"

new_repo empty_range
expect "empty range" 0 "hygiene: clean over main..topic"

new_repo en_dash
commit_file a.txt "ok
one ${en} two"
expect "en dash in changed file" 1 "a.txt:2:one ${en} two" "hygiene: failed over main..topic"

new_repo em_dash
commit_file src/a.txt "one ${em} two"
expect "em dash in changed file" 1 "src/a.txt:1:one ${em} two"

new_repo unchanged
printf 'old %s text\n' "$em" >"$repo/old.txt"
g checkout -q main
g add -f -- old.txt
g commit -q -m 'chore: a dash from before'
g checkout -q -b topic2
commit_file a.txt "plain"
range_head=topic2 expect "unchanged file ignored" 0

new_repo base_moved
g checkout -q main
commit_file later.txt "main moved on ${en} with a dash"
g checkout -q topic
commit_file a.txt "plain"
expect "a dash only on the moved base is ignored" 0

new_repo deleted
commit_file a.txt "one ${en} two"
g checkout -q main
g merge -q --ff-only topic
g checkout -q -b topic2
g rm -q a.txt
g commit -q -m 'chore: remove it'
range_head=topic2 expect "a file the range deletes is ignored" 0

new_repo renamed
commit_file a.txt "one ${en} two"
g checkout -q main
g merge -q --ff-only topic
g checkout -q -b topic2
g mv a.txt b.txt
g commit -q -m 'chore: rename it'
range_head=topic2 expect "a renamed file is checked under its new name" 1 "b.txt:1:one ${en} two"

new_repo colons
commit_file "dir/a|b&c.txt" "one ${em} two"
expect "a path with sed metacharacters is printed whole" 1 "dir/a|b&c.txt:1:one ${em} two"

new_repo nul_in_source
printf 'x\000\n%s\n' "$en" >"$repo/trojan.rs"
g add -f -- trojan.rs
g commit -q -m 'chore: nul'
expect "a NUL does not hide a dash in a source file" 1 "trojan.rs:2:${en}"

new_repo binary_format
printf '\000asm\000%s\000' "$en" >"$repo/real.wasm"
g add -f -- real.wasm
g commit -q -m 'chore: wasm'
expect "bytes in a real binary format are skipped" 0

new_repo fake_binary
printf 'just text %s\n' "$en" >"$repo/fake.wasm"
g add -f -- fake.wasm
g commit -q -m 'chore: fake wasm'
expect "text merely named like a binary is checked" 1 "fake.wasm:1:just text ${en}"

new_repo trailer
commit_file a.txt "plain" "feat: add a

${trailer}"
sha=$(g rev-parse topic)
expect "co-authored trailer" 1 "$sha:3:${trailer}"

new_repo trailer_case
commit_file a.txt "plain" "feat: add a

$(printf '%s' "$trailer" | tr '[:upper:]' '[:lower:]')"
expect "co-authored trailer in lower case" 1 "$(g rev-parse topic):3:"

new_repo generated
commit_file a.txt "plain" "feat: add a

${generated}"
expect "$generated_case" 1 "$(g rev-parse topic):3:${generated}"

new_repo regenerated
commit_file a.txt "plain" "chore: rebuild

${regenerated}"
expect "a sentence about a regenerated file passes" 0

new_repo tool_name
commit_file a.txt "plain" "feat: add a

${tool}"
expect "tool name" 1 "$(g rev-parse topic):3:${tool}"

new_repo dash_in_message
commit_file a.txt "plain" "feat: one ${em} two"
expect "em dash in a commit message" 1 "$(g rev-parse topic):1:feat: one ${em} two"

new_repo older_commit
commit_file a.txt "plain" "feat: add a

${trailer}"
bad=$(g rev-parse topic)
commit_file b.txt "plain" "feat: add b"
expect "every commit in the range is read" 1 "$bad:3:"

new_repo before_range
commit_file a.txt "plain" "feat: add a

${trailer}"
g checkout -q main
g merge -q --ff-only topic
g checkout -q -b topic2
commit_file b.txt "plain" "feat: add b"
range_head=topic2 expect "a commit before the range is not read" 0

new_repo internal
commit_file .internal/notes.md "plain"
expect "internal path tracked" 1 ".internal/notes.md: internal file tracked"

new_repo internal_old
mkdir -p "$repo/sub/$assistant_dir"
printf 'x\n' >"$repo/sub/$assistant_dir/settings.json"
g checkout -q main
g add -f -- sub
g commit -q -m 'chore: internal from before'
g checkout -q -b topic2
commit_file a.txt "plain"
range_head=topic2 expect "internal path the range did not touch" 1 "settings.json: internal file tracked"

for path in "implementation.md" "docs/implementation.md" "$assistant_file" "plan.internal.md" \
  "PRODUCT.md" "DESIGN.md" ".hooks/pre-commit"; do
  new_repo "internal_$(printf '%s' "$path" | tr '/.' '__')"
  commit_file "$path" "plain"
  expect "internal path $path" 1 "$path: internal file tracked"
done

new_repo not_internal
commit_file docs/PRODUCT.md "plain"
commit_file design/notes.md "plain"
expect "a similar path that is not internal passes" 0

new_repo every_kind
commit_file a.txt "one ${en} two" "feat: add a

${trailer}"
commit_file .internal/x.md "plain"
expect "every kind of violation in one run" 1 "a.txt:1:" "$(g rev-parse topic~1):3:" ".internal/x.md: internal file tracked"

new_repo bad_ref
range_head=no-such-ref expect "a head that does not resolve exits 2" 2 "cannot resolve no-such-ref"
range_base=no-such-ref expect "a base that does not resolve exits 2" 2 "cannot resolve no-such-ref"

new_repo usage
usage_out=$(cd "$repo" && sh "$script" main 2>&1) && usage_code=0 || usage_code=$?
if [ "$usage_code" -eq 2 ] && [[ "$usage_out" == *"usage: hygiene.sh"* ]]; then
  echo "ok: a missing argument exits 2"
  passed=$((passed + 1))
else
  echo "FAIL: a missing argument: exit $usage_code"
  failures=$((failures + 1))
fi

new_repo env_override
commit_file a.txt "plain"
other="$repo"
new_repo env_real
commit_file a.txt "one ${en} two"
GIT_DIR="$other/.git" GIT_INDEX_FILE="$other/.git/index" expect "inherited git variables are ignored" 1 "a.txt:1:one ${en} two"

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
