#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Self test for secret-scan.sh. Each case builds a throwaway repo, commits to
# it and checks the scan's exit code. Every key is generated at run time from
# random bytes, so no key shaped string is ever committed here.
set -euo pipefail

unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE

script="$(cd "$(dirname "$0")" && pwd)/secret-scan.sh"
work=$(cd "$(mktemp -d)" && pwd -P)
trap 'rm -rf "$work"' EXIT

passed=0
failures=0

random() { LC_ALL=C tr -dc "$1" </dev/urandom | head -c "$2" || true; }
# Split so the prefix and the body never sit next to each other in this file.
fake_token() { printf '%s%s' 'gh''p_' "$(random 'A-Za-z0-9' 36)"; }
fake_private_key() { printf '0x%s' "$(random '0-9a-f' 64)"; }

# repo <name>: a fresh repo with one clean commit.
repo() {
  dir="$work/$1"
  git init -q -b main "$dir"
  git -C "$dir" config user.name Test
  git -C "$dir" config user.email test@example.invalid
  git -C "$dir" config commit.gpgsign false
  git -C "$dir" config core.hooksPath /dev/null
  echo "hello" >"$dir/README.md"
  commit "$dir" "first"
}

commit() { git -C "$1" add -A && git -C "$1" commit -q --allow-empty -m "$2"; }

# expect <case name> <wanted exit code> <repo>
expect() {
  local name=$1 want=$2 got=0 out
  out=$("$script" "$3" 2>&1) || got=$?
  if [ "$got" -eq "$want" ]; then
    passed=$((passed + 1))
    echo "PASS  $name"
  else
    failures=$((failures + 1))
    echo "FAIL  $name (wanted exit $want, got $got)"
    printf '%s\n' "$out" | sed 's/^/      /'
  fi
  # A finding must never print the key, in full or in part.
  if [ -n "${secret:-}" ] && printf '%s' "$out" | grep -q -F -e "${secret:6:16}"; then
    failures=$((failures + 1))
    echo "FAIL  $name printed the key"
  fi
  secret=
}

repo clean
git -C "$work/clean" checkout -q -b side
echo "side" >"$work/clean/side.txt"
commit "$work/clean" "side"
git -C "$work/clean" checkout -q main
echo "main" >"$work/clean/main.txt"
commit "$work/clean" "main"
git -C "$work/clean" merge -q --no-edit side
commit "$work/clean" "empty"
expect "clean history with a merge and an empty commit passes" 0 "$work/clean"

repo planted
secret=$(fake_token)
echo "token = \"$secret\"" >"$work/planted/config.txt"
commit "$work/planted" "plant"
expect "planted key fails" 1 "$work/planted"

repo removed
secret=$(fake_token)
echo "token = \"$secret\"" >"$work/removed/config.txt"
commit "$work/removed" "plant"
git -C "$work/removed" rm -q config.txt
commit "$work/removed" "remove"
expect "key removed in a later commit still fails" 1 "$work/removed"

repo inline
secret=$(fake_token)
echo "token = \"$secret\" # gitleaks:allow" >"$work/inline/config.txt"
commit "$work/inline" "plant"
expect "inline gitleaks:allow does not hide a key" 1 "$work/inline"

repo ignorefile
secret=$(fake_token)
echo "token = \"$secret\"" >"$work/ignorefile/config.txt"
commit "$work/ignorefile" "plant"
sha=$(git -C "$work/ignorefile" rev-parse HEAD)
echo "$sha:config.txt:github-pat:1" >"$work/ignorefile/.gitleaksignore"
commit "$work/ignorefile" "ignore it"
expect "a .gitleaksignore does not hide a key" 1 "$work/ignorefile"

repo generic
secret=$(fake_private_key)
mkdir -p "$work/generic/test"
printf '{\n  "private_key": "%s"\n}\n' "$secret" >"$work/generic/test/keys.json"
commit "$work/generic" "plant"
expect "private key in a test file fails" 1 "$work/generic"

repo shallow
echo "two" >"$work/shallow/two.txt"
commit "$work/shallow" "two"
git clone -q --depth 1 "file://$work/shallow" "$work/shallow-clone"
expect "shallow clone is refused" 2 "$work/shallow-clone"

# The attestation parity fixture is the one allowlisted file, and only for its
# private_key lines under the generic rule.
fixture=spikes/attestation-parity/fixture.json
fixture_repo() {
  repo "$1"
  mkdir -p "$work/$1/spikes/attestation-parity"
  printf '{\n  "keys": {\n    "attester": {\n      "private_key": "%s",\n      "other": "%s"\n    }\n  }\n}\n' "$2" "$3" >"$work/$1/$fixture"
  commit "$work/$1" "fixture"
}

fixture_repo fixture-ok "$(fake_private_key)" "plain"
expect "private_key line in the parity fixture is allowed" 0 "$work/fixture-ok"

secret=$(fake_token)
fixture_repo fixture-token "$(fake_private_key)" "$secret"
expect "token in the parity fixture still fails" 1 "$work/fixture-token"

repo fixture-other-key
secret=$(fake_private_key)
mkdir -p "$work/fixture-other-key/spikes/attestation-parity"
printf '{\n  "api_key": "%s"\n}\n' "$secret" >"$work/fixture-other-key/$fixture"
commit "$work/fixture-other-key" "fixture"
expect "other key names in the parity fixture still fail" 1 "$work/fixture-other-key"

repo fixture-elsewhere
secret=$(fake_private_key)
mkdir -p "$work/fixture-elsewhere/spikes/other"
printf '{\n  "keys": {\n    "attester": {\n      "private_key": "%s",\n    }\n  }\n}\n' "$secret" >"$work/fixture-elsewhere/spikes/other/fixture.json"
commit "$work/fixture-elsewhere" "fixture"
expect "the same line in another file fails" 1 "$work/fixture-elsewhere"

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
