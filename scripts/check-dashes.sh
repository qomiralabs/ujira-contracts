#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Shared file: the same copy lives in ujira, ujira-contracts, ujira-docs and
# ujira-mcp (qomiralabs/ujira#303). Change all four together.
#
# Fails if any tracked file contains an em dash (U+2014) or an en dash
# (U+2013). It searches the content git holds for each tracked file (the
# index, which is the committed content plus anything staged), not the working
# tree, so untracked files are not checked. Each hit is printed as
# path:line:text.
#
# Every file is read as text, so a NUL byte cannot make a file look binary and
# skip the check. A file is skipped only when its extension is one of the
# formats below and its content starts with that format's signature, so a
# dash in a file merely named like an image is still found. The bytes of a
# real image can spell a dash by chance, which is why those are skipped.
#
# Fails closed: if git cannot find the repository or search it, the check
# fails rather than passing.
#
# Usage: scripts/check-dashes.sh
# Exit codes: 0 no dashes, 1 dashes found, 2 the repository could not be
# searched.
set -euo pipefail

# Any GIT_ variable could point git at another repository, index or config, so
# none is inherited.
while IFS= read -r name; do
  [ -n "$name" ] || continue
  unset "$name"
done <<<"$(env | sed -n 's/^\(GIT_[A-Za-z0-9_]*\)=.*/\1/p')"

if ! top=$(git rev-parse --show-toplevel 2>/dev/null) || [ -z "$top" ]; then
  echo "check-dashes: could not find the git repository" >&2
  exit 2
fi

# The two characters, written as raw UTF-8 bytes so this works with BSD and
# GNU tools.
em=$(printf '\xe2\x80\x94')
en=$(printf '\xe2\x80\x93')

# Binary formats whose files are skipped, with the hex signature each must
# start with. Keep this list short and in step with the invisible character
# check.
binary_signature() {
  case ${1##*.} in
    png | PNG) printf '89504e470d0a1a0a' ;;
    jpg | jpeg | JPG | JPEG) printf 'ffd8ff' ;;
    gif | GIF) printf '474946383' ;;
    webp | WEBP) printf '52494646' ;;
    ico | ICO) printf '00000100' ;;
    woff | WOFF) printf '774f4646' ;;
    woff2 | WOFF2) printf '774f4632' ;;
    ttf | TTF) printf '0001000000' ;;
    otf | OTF) printf '4f54544f' ;;
    wasm | WASM) printf '0061736d' ;;
    pdf | PDF) printf '25504446' ;;
    *) printf '' ;;
  esac
}

# True when the file is really one of those formats: the extension is listed
# and the content starts with its signature.
is_real_binary() {
  local want head
  want=$(binary_signature "$1")
  [ -n "$want" ] || return 1
  head=$(git -C "$top" show ":$1" 2>/dev/null | od -An -v -tx1 -N 8 | tr -d ' \n')
  case "$head" in
    "$want"*) return 0 ;;
    *) return 1 ;;
  esac
}

# --text so nothing is skipped as binary here; -z so a path containing a
# colon or a space is still read whole. The hits go to a file, not a
# variable, because a shell variable cannot hold the NUL separators.
hits=$(mktemp)
trap 'rm -f "$hits"' EXIT

code=0
LC_ALL=C git -C "$top" -c core.quotePath=false grep --cached --text -z -n -F \
  -e "$em" -e "$en" -- . >"$hits" || code=$?

if [ "$code" -gt 1 ]; then
  echo "check-dashes: git could not search the repository (exit $code)" >&2
  exit 2
fi

found=0
while IFS= read -r -d '' file && IFS= read -r -d '' line && IFS= read -r text; do
  if is_real_binary "$file"; then continue; fi
  printf '%s:%s:%s\n' "$file" "$line" "$text"
  found=1
done <"$hits"

if [ "$found" -ne 0 ]; then
  echo "Replace em and en dashes with a comma, a colon, parentheses or the word \"to\"." >&2
  exit 1
fi
echo "No em or en dashes found."
