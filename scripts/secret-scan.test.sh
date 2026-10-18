#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Self test for secret-scan.sh. Each case builds a throwaway repo, commits to
# it and checks the scan's exit code. Every key is generated at run time from
# random bytes, so no key shaped string is ever committed here.
set -euo pipefail

unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE SECRET_SCAN_BASE_REF

here="$(cd "$(dirname "$0")/.." && pwd)"
script="$here/scripts/secret-scan.sh"
work=$(cd "$(mktemp -d)" && pwd -P)
trap 'rm -rf "$work"' EXIT

passed=0
failures=0
secret=
base_ref=
want_out=

random() { LC_ALL=C tr -dc "$1" </dev/urandom | head -c "$2" || true; }
# Split so the prefix and the body never sit next to each other in this file.
fake_token() { printf '%s%s' 'gh''p_' "$(random 'A-Za-z0-9' 36)"; }
fake_private_key() { printf '0x%s' "$(random '0-9a-f' 64)"; }

commit() { git -C "$1" add -A && git -C "$1" commit -q --allow-empty -m "$2"; }

# repo <name>: a fresh repo with one clean commit.
repo() {
  dir="$work/$1"
  git init -q -b main "$dir"
  git -C "$dir" config user.name Test
  git -C "$dir" config user.email test@example.invalid
  git -C "$dir" config commit.gpgsign false
  git -C "$dir" config core.hooksPath /dev/null
  echo "hello" >"$dir/README.md"
  printf '\000\001\002' >"$dir/blob.bin"
  commit "$dir" "first"
}

# plant <name> <path>: a repo whose second commit puts a fake token at path.
plant() {
  repo "$1"
  secret=$(fake_token)
  mkdir -p "$(dirname "$work/$1/$2")"
  echo "token = \"$secret\"" >"$work/$1/$2"
  commit "$work/$1" "plant"
}

# expect <case name> <wanted exit code> <repo>
expect() {
  local name=$1 want=$2 got=0 out
  if [ -n "$base_ref" ]; then
    out=$(SECRET_SCAN_BASE_REF="$base_ref" "$script" "$3" 2>&1) || got=$?
  else
    out=$("$script" "$3" 2>&1) || got=$?
  fi
  if [ "$got" -eq "$want" ] && { [ -z "$want_out" ] || printf '%s' "$out" | grep -q -F -e "$want_out"; }; then
    passed=$((passed + 1))
    echo "PASS  $name"
  else
    failures=$((failures + 1))
    echo "FAIL  $name (wanted exit $want${want_out:+ and output $want_out}, got $got)"
    printf '%s\n' "$out" | sed 's/^/      /'
  fi
  # A finding must never print the key, in full or in part.
  if [ -n "$secret" ] && printf '%s' "$out" | grep -q -F -e "${secret:6:16}"; then
    failures=$((failures + 1))
    echo "FAIL  $name printed the key"
  fi
  secret=
  base_ref=
  want_out=
}

# Clean histories, including commits that add no text, must pass the
# completeness check.
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

repo delete-only
git -C "$work/delete-only" rm -q README.md
commit "$work/delete-only" "delete"
expect "delete only commit passes" 0 "$work/delete-only"

repo rename-only
git -C "$work/rename-only" mv README.md READ.md
commit "$work/rename-only" "rename"
expect "rename only commit passes" 0 "$work/rename-only"

repo mode-only
chmod +x "$work/mode-only/README.md"
commit "$work/mode-only" "mode"
expect "mode only commit passes" 0 "$work/mode-only"

repo binary-only
printf '\000\005' >"$work/binary-only/more.bin"
commit "$work/binary-only" "binary"
expect "binary only commit passes" 0 "$work/binary-only"

repo symlink
ln -s README.md "$work/symlink/link"
commit "$work/symlink" "symlink"
expect "symlink commit passes" 0 "$work/symlink"

repo removal
: >"$work/removal/README.md"
touch "$work/removal/empty.txt"
commit "$work/removal" "empty a file and add an empty one"
expect "commit that only removes lines or adds an empty file passes" 0 "$work/removal"

repo merge-delete
git -C "$work/merge-delete" checkout -q -b side
git -C "$work/merge-delete" rm -q README.md
commit "$work/merge-delete" "side"
git -C "$work/merge-delete" checkout -q main
git -C "$work/merge-delete" merge -q --no-ff --no-edit side
expect "merge that only deletes passes" 0 "$work/merge-delete"

# Keys must be found wherever they sit in history.
plant planted config.txt
expect "planted key fails" 1 "$work/planted"

plant removed config.txt
git -C "$work/removed" rm -q config.txt
commit "$work/removed" "remove"
expect "key removed in a later commit still fails" 1 "$work/removed"

repo inline
secret=$(fake_token)
echo "token = \"$secret\" # gitleaks:allow" >"$work/inline/config.txt"
commit "$work/inline" "plant"
expect "inline gitleaks:allow does not hide a key" 1 "$work/inline"

plant ignorefile config.txt
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

# A merge resolution can add content that is in neither parent.
repo evil-merge
git -C "$work/evil-merge" checkout -q -b side
echo "side" >"$work/evil-merge/side.txt"
commit "$work/evil-merge" "side"
git -C "$work/evil-merge" checkout -q main
echo "main" >"$work/evil-merge/main.txt"
commit "$work/evil-merge" "main"
git -C "$work/evil-merge" merge -q --no-ff --no-commit side
secret=$(fake_token)
echo "token = \"$secret\"" >"$work/evil-merge/merged.txt"
git -C "$work/evil-merge" add -A
git -C "$work/evil-merge" commit -q --no-edit
expect "key added only in a merge resolution fails" 1 "$work/evil-merge"

# git attributes must not turn a text file into an unscanned binary diff.
repo attr-diff
echo "cfg.txt -diff" >"$work/attr-diff/.gitattributes"
secret=$(fake_token)
echo "token = \"$secret\"" >"$work/attr-diff/cfg.txt"
commit "$work/attr-diff" "plant"
expect "key in a file marked -diff fails" 1 "$work/attr-diff"

repo attr-binary
echo "*.cfg binary" >"$work/attr-binary/.gitattributes"
secret=$(fake_token)
echo "token = \"$secret\"" >"$work/attr-binary/app.cfg"
commit "$work/attr-binary" "plant"
expect "key in a file marked binary fails" 1 "$work/attr-binary"

# Paths that the upstream default config skips are scanned here: gitleaks
# itself must report the key, not only the key file check below.
n=0
for path in pnpm-lock.yaml package-lock.json yarn.lock node_modules/pkg/index.js docs/logo.svg \
  img/photo.png go.sum a/b/gitleaks.toml .gitleaks.toml certs/server.pem certs/server.key logs/app.log; do
  n=$((n + 1))
  plant "skip-$n" "$path"
  want_out="RuleID:"
  expect "key in $path is reported by gitleaks" 1 "$work/skip-$n"
done

# Key files are refused by name anywhere in history, binary or not, since
# their content cannot be line scanned.
n=0
for path in certs/client.p12 certs/client.pfx keystore/app.jks deploy/id_rsa home/.ssh/id_ed25519 certs/tls.KEY; do
  n=$((n + 1))
  repo "keyfile-$n"
  mkdir -p "$(dirname "$work/keyfile-$n/$path")"
  printf '\060\202\001\000%s' "$(random 'A-Za-z0-9' 16)" >"$work/keyfile-$n/$path"
  commit "$work/keyfile-$n" "add $path"
  want_out="key file"
  expect "$path is refused" 1 "$work/keyfile-$n"
done

repo keyfile-removed
mkdir -p "$work/keyfile-removed/certs"
printf '\060\202\001\000' >"$work/keyfile-removed/certs/old.p12"
commit "$work/keyfile-removed" "add"
git -C "$work/keyfile-removed" rm -q certs/old.p12
commit "$work/keyfile-removed" "remove"
want_out="key file"
expect "key file removed in a later commit is still refused" 1 "$work/keyfile-removed"

repo keylike-names
mkdir -p "$work/keylike-names/src"
echo "x" >"$work/keylike-names/src/keys.ts"
echo "x" >"$work/keylike-names/src/monkey.txt"
echo "x" >"$work/keylike-names/src/keyboard.key.md"
commit "$work/keylike-names" "names"
expect "names that only contain key words pass" 0 "$work/keylike-names"

repo shallow
echo "two" >"$work/shallow/two.txt"
commit "$work/shallow" "two"
git clone -q --depth 1 "file://$work/shallow" "$work/shallow-clone"
expect "shallow clone is refused" 2 "$work/shallow-clone"

# Pull request runs take the config from the base branch, so a change cannot
# loosen the rules its own scan runs with.
repo pr-widen
cp "$here/.gitleaks.toml" "$work/pr-widen/.gitleaks.toml"
commit "$work/pr-widen" "base config"
git -C "$work/pr-widen" update-ref refs/remotes/origin/development HEAD
printf '[extend]\nuseDefault = false\n' >"$work/pr-widen/.gitleaks.toml"
secret=$(fake_token)
echo "token = \"$secret\"" >"$work/pr-widen/config.txt"
commit "$work/pr-widen" "widen and plant"
base_ref=development
expect "a pull request cannot empty its own rules" 1 "$work/pr-widen"

plant pr-no-base config.txt
git -C "$work/pr-no-base" update-ref refs/remotes/origin/development HEAD~1
base_ref=development
expect "base without a config falls back to the default rules" 1 "$work/pr-no-base"

repo pr-clean
git -C "$work/pr-clean" update-ref refs/remotes/origin/development HEAD
echo "more" >"$work/pr-clean/more.txt"
commit "$work/pr-clean" "more"
base_ref=development
expect "clean pull request with the fallback rules passes" 0 "$work/pr-clean"

repo pr-missing-base
base_ref=development
expect "missing base branch is refused" 2 "$work/pr-missing-base"

# The attestation parity fixture is the one allowlisted file, and only for
# the two exact public test keys it holds, on its private_key lines, under the
# generic rule. The real values are read from the committed fixture at run
# time so this file holds nothing key shaped.
fixture=spikes/attestation-parity/fixture.json
real_key() {
  python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["keys"][sys.argv[2]]["private_key"])' "$here/$fixture" "$1"
}
attester_key=$(real_key attester)
other_key=$(real_key other_signer)

# fixture_repo <name> <attester key> <other signer key> <extra value>
fixture_repo() {
  repo "$1"
  mkdir -p "$work/$1/spikes/attestation-parity"
  printf '{\n  "keys": {\n    "attester": {\n      "private_key": "%s",\n      "other": "%s"\n    },\n    "other_signer": {\n      "private_key": "%s",\n    }\n  }\n}\n' "$2" "$4" "$3" >"$work/$1/$fixture"
  commit "$work/$1" "fixture"
}

fixture_repo fixture-ok "$attester_key" "$other_key" "plain"
expect "the two public test keys in the parity fixture are allowed" 0 "$work/fixture-ok"

secret=$(fake_private_key)
fixture_repo fixture-random "$attester_key" "$secret" "plain"
expect "any other key on a private_key line of the parity fixture fails" 1 "$work/fixture-random"

secret=$(fake_token)
fixture_repo fixture-token "$attester_key" "$other_key" "$secret"
expect "token in the parity fixture still fails" 1 "$work/fixture-token"

repo fixture-other-key
secret=$(fake_private_key)
mkdir -p "$work/fixture-other-key/spikes/attestation-parity"
printf '{\n  "api_key": "%s"\n}\n' "$secret" >"$work/fixture-other-key/$fixture"
commit "$work/fixture-other-key" "fixture"
expect "other key names in the parity fixture still fail" 1 "$work/fixture-other-key"

repo fixture-elsewhere
mkdir -p "$work/fixture-elsewhere/spikes/other"
printf '{\n  "keys": {\n    "attester": {\n      "private_key": "%s",\n    }\n  }\n}\n' "$attester_key" >"$work/fixture-elsewhere/spikes/other/fixture.json"
commit "$work/fixture-elsewhere" "fixture"
expect "a public test key in any other file fails" 1 "$work/fixture-elsewhere"

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
