#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Self test for build-smart-account.sh: the refusals that happen before any
# build, so it needs no network and no toolchain. The real build, and the hash
# it must give, is checked by running the script itself.
set -uo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
script="$root/scripts/build-smart-account.sh"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

pin=ddc327eb92d63709436da3a8455f641c2d5476c5
passed=0
failures=0

expect() {
  local name=$1 want=$2 needle=$3 out code=0
  shift 3
  out=$("$@" 2>&1) || code=$?
  if [ "$code" -ne "$want" ]; then
    echo "FAIL: $name: exit $code, want $want"
    printf '%s\n' "$out" | sed 's/^/    /'
    failures=$((failures + 1))
  elif ! grep -qF -- "$needle" <<<"$out"; then
    echo "FAIL: $name: output does not mention '$needle'"
    printf '%s\n' "$out" | sed 's/^/    /'
    failures=$((failures + 1))
  else
    echo "ok: $name"
    passed=$((passed + 1))
  fi
}

# A local repository whose v0.8.0 tag sits on some other commit, as a moved tag
# would, so the pin check runs against something it must refuse.
moved="$work/moved"
git -c init.defaultBranch=main init -q "$moved"
git -C "$moved" -c user.name=t -c user.email=t@example.com commit -q --allow-empty -m "not the audited commit"
git -C "$moved" tag v0.8.0

# The same repository with the tag missing.
untagged="$work/untagged"
git -c init.defaultBranch=main init -q "$untagged"
git -C "$untagged" -c user.name=t -c user.email=t@example.com commit -q --allow-empty -m "no tags here"

# A stellar binary of the wrong version, reached only once the pin passes, so
# it needs a repository that answers with the pinned commit for the tag.
stub="$work/stellar"
printf '#!/bin/sh\necho "stellar 0.0.1 (stub)"\n' >"$stub"
chmod +x "$stub"
git_stub="$work/bin"
mkdir -p "$git_stub"
real_git=$(command -v git)
cat >"$git_stub/git" <<EOF
#!/bin/sh
if [ "\$1" = "ls-remote" ]; then
  printf '%s\trefs/tags/v0.8.0\n' "$pin"
  exit 0
fi
exec "$real_git" "\$@"
EOF
chmod +x "$git_stub/git"

expect "no arguments" 2 "usage:" "$script"
expect "too many arguments" 2 "usage:" "$script" v0.8.0 v0.8.0
expect "--out-dir without a directory" 2 "usage:" "$script" --out-dir v0.8.0
expect "unknown option in place of a tag" 2 "is not a pinned tag" "$script" --frobnicate
expect "a tag that is not pinned" 2 "is not a pinned tag" "$script" v0.7.3
expect "a moved tag is refused" 1 "but is pinned to $pin" env OZ_REPO="$moved" "$script" v0.8.0
expect "a missing tag is refused" 1 "is missing upstream" env OZ_REPO="$untagged" "$script" v0.8.0
expect "an unreachable repository" 2 "cannot read the tags" env OZ_REPO="$work/does-not-exist" "$script" v0.8.0
expect "the wrong stellar version is refused" 1 "is not the pinned" env PATH="$git_stub:$PATH" STELLAR="$stub" "$script" v0.8.0

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
