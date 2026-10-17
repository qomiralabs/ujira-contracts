#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Self test for check-invisible-characters.py. Each case builds a throwaway git
# repo in a temp directory, stages some files and checks the exit code and
# output. Every planted character is produced from its code point at run time,
# so this file holds none of them literally.
set -euo pipefail

script="$(cd "$(dirname "$0")" && pwd)/check-invisible-characters.py"
work=$(cd "$(mktemp -d)" && pwd -P)
trap 'rm -rf "$work"' EXIT

passed=0
failures=0

new_repo() {
  repo="$work/$1"
  mkdir -p "$repo"
  git -C "$repo" init -q
}

# char <hex code point>: the character as UTF-8 bytes
char() {
  python3 -c 'import sys; sys.stdout.buffer.write(chr(int(sys.argv[1], 16)).encode())' "$1"
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
expect "empty tree passes" 0 "0 text file(s) checked"

new_repo clean
write soroban/src/lib.rs "fn main() {}"
write README.md "plain text, caf$(char e9) and $(char 4e2d)$(char 6587) are fine"
expect "ordinary text passes" 0 "2 text file(s) checked"

# Format, line separator and paragraph separator characters, rejected in
# every file, and the lookalikes, rejected outside the allowlist.
for point in FEFF 200B 200C 200D 200E 200F 202A 202B 202C 202D 202E \
  2066 2067 2068 2069 2028 2029 00AD 2060 E0001 00A0 202F 2212; do
  new_repo "planted_$point"
  write celo/src/Planted.sol "ok
x$(char "$point")"
  expect "planted U+$point fails" 1 "celo/src/Planted.sol:2:2: U+$point"
done

new_repo columns
write a.rs "$(char 1F600)$(char 200B)

  $(char 2060)$(char 2060)"
expect "columns count code points and every occurrence is named" 1 \
  "a.rs:1:2: U+200B" "a.rs:3:3: U+2060" "a.rs:3:4: U+2060"

new_repo allowlist
write docs/typeset.md "a$(char A0)b"
write docs/other.md "a$(char A0)b"
write docs/bidi.md "a$(char 202E)b"
write scripts/invisible-characters-allowlist.txt "# Typeset prose
docs/typeset.md
docs/bidi.md"
expect "lookalikes pass only in allowlisted files" 1 "docs/other.md:1:2: U+00A0" "docs/bidi.md:1:2: U+202E"
if "$script" "$repo" 2>&1 | grep -qF "docs/typeset.md"; then
  echo "FAIL: allowlisted file was reported"
  failures=$((failures + 1))
else
  echo "ok: allowlisted file is not reported"
  passed=$((passed + 1))
fi

new_repo binary
printf '\x89PNG\x00%s' "$(char 202E)" >"$repo/image.txt"
git -C "$repo" add -f -- image.txt
write a.rs "fn a() {}"
expect "binary content is skipped whatever the extension" 0 "1 text file(s) checked, 1 binary file(s) skipped"

new_repo invalid
printf 'caf\xe9\n' >"$repo/latin1.txt"
git -C "$repo" add -f -- latin1.txt
expect "invalid UTF-8 fails" 1 "latin1.txt: not valid UTF-8 text"

new_repo missing
write gone.rs "fn a() {}"
rm "$repo/gone.rs"
expect "tracked file missing from the tree fails" 1 "gone.rs: tracked but missing from the working tree"

new_repo hostile_name
write "$(printf 'bad\nname.rs')" "$(char 200B)"
expect "hostile file names are escaped" 1 '"bad\x0aname.rs":1:1: U+200B'

new_repo symlink
printf '%s\n' "$(char 202E)" >"$work/outside.txt"
ln -s "$work/outside.txt" "$repo/link.txt"
git -C "$repo" add -- link.txt
expect "symlinks are not followed" 0 "0 text file(s) checked"

new_repo subfolder_root
write a.rs "$(char 200B)"
mkdir -p "$repo/celo"
target="$repo/celo"
expect "subfolder root resolves to the top level" 1 "a.rs:1:1: U+200B"
unset target

mkdir -p "$work/not_a_repo"
target="$work/not_a_repo"
GIT_CEILING_DIRECTORIES=$work expect "root outside a git repo exits 2" 2 "not inside a git work tree"
unset target

new_repo other_repo
write a.rs "fn a() {}"
other="$repo"
new_repo git_dir_override
write a.rs "$(char 200B)"
GIT_DIR="$other/.git" GIT_WORK_TREE="$other" GIT_INDEX_FILE="$other/.git/index" \
  expect "inherited GIT_DIR does not redirect the check" 1 "a.rs:1:1: U+200B"

new_repo git_fails
write a.rs "fn a() {}"
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
