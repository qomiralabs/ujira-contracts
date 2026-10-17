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
    # A pattern match, not a pipe into grep -q, so pipefail cannot turn an
    # early grep exit into a false failure.
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
allowlist_out=$("$script" "$repo" 2>&1) || true
if [[ "$allowlist_out" == *"docs/typeset.md"* ]]; then
  echo "FAIL: allowlisted file was reported"
  failures=$((failures + 1))
else
  echo "ok: allowlisted file is not reported"
  passed=$((passed + 1))
fi

# A NUL byte does not make a source file binary: it is reported and the rest
# of the file is still checked.
new_repo nul_in_source
printf '// x\000\n%s\n' "$(char 202E)" >"$repo/trojan.rs"
git -C "$repo" add -f -- trojan.rs
expect "a NUL in a source file is reported and the file is still checked" 1 \
  "trojan.rs:1:5: U+0000" "trojan.rs:2:1: U+202E"

new_repo nul_unlisted
printf '\000\001\002' >"$repo/blob.dat"
git -C "$repo" add -f -- blob.dat
expect "binary content under an unlisted extension is checked as text" 1 "blob.dat:1:1: U+0000"

# Only listed extensions are skipped, and only when the content starts with
# the format's signature. Each signature is written out here independently of
# the script, as printf escapes.
for entry in \
  'png:\211PNG\r\n\032\n' 'jpg:\377\330\377' 'jpeg:\377\330\377' \
  'gif:GIF87a' 'gif:GIF89a' 'webp:RIFF\000\000\000\000WEBP' 'ico:\000\000\001\000' \
  'woff:wOFF' 'woff2:wOF2' 'wasm:\000asm' 'pdf:%%PDF-'; do
  ext=${entry%%:*}
  signature=${entry#*:}
  new_repo "binary_$ext"
  # shellcheck disable=SC2059 # the signature is a printf format on purpose
  printf "$signature" >"$repo/asset.$ext"
  printf '\000%s' "$(char 202E)" >>"$repo/asset.$ext"
  # shellcheck disable=SC2059
  printf "$signature" >"$repo/UPPER.$(printf '%s' "$ext" | tr '[:lower:]' '[:upper:]')"
  git -C "$repo" add -f -- .
  expect ".$ext with its signature is skipped" 0 "0 text file(s) checked, 2 binary file(s) skipped"

  new_repo "mismatch_$ext"
  printf 'not really\n' >"$repo/asset.$ext"
  git -C "$repo" add -f -- "asset.$ext"
  expect ".$ext without its signature fails" 1 "asset.$ext: named .$ext but its content is not $ext"
done

new_repo webp_not_webp
printf 'RIFF\000\000\000\000WAVE' >"$repo/sound.webp"
git -C "$repo" add -f -- sound.webp
expect "a RIFF file that is not WebP fails as .webp" 1 "sound.webp: named .webp but its content is not webp"

new_repo dotted_folder
write "assets.png/readme" "ok"
expect "a dot in a folder name is not an extension" 0 "1 text file(s) checked"

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
