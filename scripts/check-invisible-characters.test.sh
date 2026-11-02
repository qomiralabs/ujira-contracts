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
extra_args=()

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

# add_unconverted <path>: stage the file's exact bytes, whatever the attributes
# or the machine's core.autocrlf would do to them
add_unconverted() {
  local oid
  oid=$(git -C "$repo" hash-object -w --no-filters -- "$1")
  git -C "$repo" update-index --add --cacheinfo "100644,$oid,$1"
}

# expect <case name> <exit code> [text the output must contain]...
# Runs the check against $target if set, otherwise against $repo.
expect() {
  local name=$1 want=$2
  shift 2
  local out code=0 needle
  out=$("$script" ${extra_args[@]+"${extra_args[@]}"} "${target:-$repo}" 2>&1) || code=$?
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
  write soroban/src/planted.rs "ok
x$(char "$point")"
  expect "planted U+$point fails" 1 "soroban/src/planted.rs:2:2: U+$point"
done

# Invisible characters outside those categories, each end of every range.
for point in 3164 115F 1160 FFA0 FE00 FE0F E0100 E01EF 2800 034F 17B4 180B FFF0 E0FFF; do
  new_repo "filler_$point"
  write soroban/src/planted.rs "ok
x$(char "$point")y"
  expect "invisible U+$point fails" 1 "soroban/src/planted.rs:2:2: U+$point"
done

# Control characters other than tab, line feed and a CRLF pair: ESC, backspace,
# a lone carriage return, NEL, and the rest of each block's ends.
for point in 0001 0008 000B 000C 000D 001B 001F 007F 0080 0085 009F; do
  new_repo "control_$point"
  write soroban/src/planted.rs "ok
x$(char "$point")y"
  expect "control U+$point fails" 1 "soroban/src/planted.rs:2:2: U+$point"
done

new_repo final_cr
printf 'ok\r' >"$repo/final.rs"
git -C "$repo" add -f -- final.rs
expect "a carriage return ending the file fails" 1 "final.rs:1:3: U+000D"

new_repo line_endings
printf 'a\tb\r\nc\n' >"$repo/ok.rs"
add_unconverted ok.rs
expect "tab, line feed and a CRLF pair pass" 0 "1 text file(s) checked"

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

  new_repo "tail_$ext"
  # shellcheck disable=SC2059
  printf "$signature" >"$repo/asset.$ext"
  printf 'hidden text %s here\n' "$(char 202E)" >>"$repo/asset.$ext"
  git -C "$repo" add -f -- "asset.$ext"
  expect "$ext signature followed by text with a bidi control" 1 "U+202E in text after the binary signature"

  new_repo "mixed_tail_$ext"
  # shellcheck disable=SC2059
  printf "$signature" >"$repo/asset.$ext"
  printf '\000\377\000hidden text %s here\000\377' "$(char 202E)" >>"$repo/asset.$ext"
  git -C "$repo" add -f -- "asset.$ext"
  expect "$ext with binary data around hidden text" 1 "U+202E in text after the binary signature"

  new_repo "mismatch_$ext"
  printf 'not really\n' >"$repo/asset.$ext"
  git -C "$repo" add -f -- "asset.$ext"
  expect ".$ext without its signature fails" 1 "asset.$ext: named .$ext but its content is not $ext"
done

new_repo tail_offset
printf '\000asm\000\377hidden text %s\000' "$(char 200B)" >"$repo/a.wasm"
git -C "$repo" add -f -- a.wasm
expect "the byte offset of hidden text is named" 1 "a.wasm: byte 18: U+200B in text after the binary signature"

new_repo tail_short_run
printf '\000asm\000\377%s\377abc%s\000' "$(char AD)" "$(char 85)" >"$repo/a.wasm"
git -C "$repo" add -f -- a.wasm
expect "a short run in binary data is not read as text" 0 "1 binary file(s) skipped"

new_repo tail_lone
printf '\000asm%s' "$(char 200B)" >"$repo/a.wasm"
git -C "$repo" add -f -- a.wasm
expect "a lone character in a tail of pure text fails" 1 "a.wasm: byte 4: U+200B"

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

new_repo name_filler
write "x$(char 3164).txt" "fine"
expect "file name with U+3164" 1 "file name: U+3164"

new_repo name_bidi
write "src/x$(char 202E)y/a.txt" "fine"
expect "file name with a bidi control in a folder" 1 "file name: U+202E"

new_repo name_tab
write "$(printf 'a\tb.txt')" "fine"
expect "file name with a tab" 1 "file name: U+0009"

new_repo name_lookalike
write "a$(char A0)b.txt" "fine"
write scripts/invisible-characters-allowlist.txt "a$(char A0)b.txt"
expect "the allowlist does not cover a file name" 1 "file name: U+00A0"

new_repo name_invalid
: >"$repo/$(printf 'caf\351.txt')"
git -C "$repo" add -f -- .
expect "file name that is not UTF-8" 1 'file name is not valid UTF-8'

new_repo symlink_target
ln -s "x$(char 200B)y" "$repo/link"
git -C "$repo" add -- link
expect "symlink target with U+200B" 1 "link: symlink target: U+200B"

new_repo symlink_target_invalid
ln -s "$(printf 'caf\351')" "$repo/link"
git -C "$repo" add -- link
expect "symlink target that is not UTF-8" 1 "link: symlink target is not valid UTF-8"

# Committed blobs, not the working tree. UTF-7 turns U+202E into plain ASCII in
# the checkout, so a check that read the working tree would see nothing.
new_repo utf7
printf 'a.txt working-tree-encoding=UTF-7\n' >"$repo/.gitattributes"
git -C "$repo" add -f -- .gitattributes
python3 -c 'import sys; open(sys.argv[1], "wb").write("x\u202ey\n".encode("utf-7"))' "$repo/a.txt"
git -C "$repo" add -f -- a.txt
expect "working-tree-encoding hides a bidi control in the checkout" 2 \
  "a.txt:1:2: U+202E" "a.txt: attribute working-tree-encoding=UTF-7 converts"

new_repo dirty_clean
write a.rs "x$(char 200B)"
printf 'clean now\n' >"$repo/a.rs"
expect "the staged blob is checked when the working tree is clean" 1 "a.rs:1:2: U+200B"

new_repo clean_dirty
write a.rs "clean"
printf 'x%s\n' "$(char 200B)" >"$repo/a.rs"
expect "a dirty working tree does not change the result" 0 "1 text file(s) checked"

new_repo allowlist_blob
write a.md "a$(char A0)b"
write scripts/invisible-characters-allowlist.txt "# nothing listed"
printf 'a.md\n' >"$repo/scripts/invisible-characters-allowlist.txt"
expect "the allowlist is read from the blob too" 1 "a.md:1:2: U+00A0"

# Attributes that convert content are refused, naming the path and attribute.
refused_by() {
  new_repo "$1"
  printf 'a.txt %s\n' "$2" >"$repo/.gitattributes"
  git -C "$repo" add -f -- .gitattributes
  write a.txt "plain"
  expect "$3" 2 "a.txt: attribute $4 converts"
}
refused_by attr_encoding "working-tree-encoding=UTF-16LE" "working-tree-encoding is refused" "working-tree-encoding=UTF-16LE"
refused_by attr_filter "filter=hide" "filter is refused" "filter=hide"
refused_by attr_ident "ident" "ident is refused" "ident"
refused_by attr_crlf "text eol=crlf" "eol=crlf on a file with line feeds is refused" "text=set eol=crlf"

new_repo attr_normalize
printf 'a.txt text\n' >"$repo/.gitattributes"
git -C "$repo" add -f -- .gitattributes
printf 'x\r\n' >"$repo/a.txt"
add_unconverted a.txt
expect "text on a blob with CRLF is refused" 2 "a.txt: attribute text=set eol=unspecified converts"

new_repo attr_harmless
printf 'a.txt text\nb.txt eol=lf\nc.txt -text\nd.txt -filter -ident\n' >"$repo/.gitattributes"
git -C "$repo" add -f -- .gitattributes
write a.txt "plain"
write b.txt "plain"
write c.txt "plain"
write d.txt "plain"
expect "attributes that leave the bytes alone pass" 0 "5 text file(s) checked"

# The Unicode version the tables must be.
running=$(python3 -c 'import unicodedata; print(unicodedata.unidata_version)')
new_repo pin
write a.rs "fine"
extra_args=(--unicode-version "$running")
expect "the running Unicode version passes and is printed" 0 "Unicode $running, as pinned"
extra_args=(--unicode-version 0.0.0)
expect "any other Unicode version exits 2" 2 "this Python has Unicode $running, but 0.0.0 is pinned"
extra_args=()

new_repo subfolder_root
write a.rs "$(char 200B)"
mkdir -p "$repo/soroban"
target="$repo/soroban"
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

new_repo elsewhere
write a.rs "fn a() {}"
elsewhere="$repo"
new_repo git_env
write a.rs "$(char 200B)"
GIT_DIR="$elsewhere/.git" GIT_INDEX_FILE="$elsewhere/.git/index" GIT_OBJECT_DIRECTORY="$elsewhere/.git/objects" \
  expect "GIT_DIR and GIT_INDEX_FILE pointing elsewhere" 1 "a.rs:1:1: U+200B"

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

fake_cat_file() {
  mkdir -p "$work/fakecat"
  {
    # shellcheck disable=SC2016 # these lines are written out for the fake git
    printf '#!/usr/bin/env bash\nfor arg in "$@"; do\n  if [ "$arg" = cat-file ]; then\n    cat >/dev/null\n'
    printf '%s\n' "$1"
    printf '    exit 0\n  fi\ndone\nexec "%s" "$@"\n' "$real_git"
  } >"$work/fakecat/git"
  chmod +x "$work/fakecat/git"
}

new_repo cat_mismatch
write a.rs "fn a() {}"
fake_cat_file "    printf '%040d blob 3\\nabc\\n' 0"
PATH="$work/fakecat:$PATH" expect "cat-file record with a different object id exits 2" 2 "git cat-file answered"

new_repo cat_short
write a.rs "fn a() {}"
oid=$(git -C "$repo" rev-parse :a.rs)
fake_cat_file "    printf '$oid blob 90\\nabc\\n'"
PATH="$work/fakecat:$PATH" expect "cat-file short record exits 2" 2 "short record"

new_repo cat_missing
write a.rs "fn a() {}"
oid=$(git -C "$repo" rev-parse :a.rs)
fake_cat_file "    printf '$oid missing\\n'"
PATH="$work/fakecat:$PATH" expect "cat-file missing object exits 2" 2 "git cat-file answered"

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
