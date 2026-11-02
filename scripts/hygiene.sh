#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# The server side of the local guard hooks, which can be skipped or missing on
# a fresh machine. Over the range from a base to a head it fails when:
#   - a file the range changes holds an en dash or an em dash, matched as raw
#     UTF-8 bytes in the head's committed content. Files the range does not
#     touch are ignored; scripts/check-dashes.sh covers the whole tree;
#   - a commit message in the range holds an attribution trailer or line, a tool
#     or model name, or a dash;
#   - any path tracked at the head matches the internal path patterns, whether
#     or not the range touched it.
# Every violation is printed: path:line:text for a file, sha:line:text for a
# commit message, and the path for a tracked internal file.
#
# The dash characters are built with printf and the patterns are assembled from
# fragments, so this file does not trip the guard it mirrors. POSIX shell plus
# git, and the patterns work with BSD and GNU grep.
#
# Usage: scripts/hygiene.sh <base> <head>
# Exit codes: 0 clean, 1 violations, 2 bad usage or a ref git cannot resolve.
set -eu

if [ "$#" -ne 2 ]; then
  echo "usage: hygiene.sh <base> <head>" >&2
  exit 2
fi
base=$1
head=$2

# Any GIT_ variable could point git at another repository, index or config, so
# none is inherited.
for name in $(env | sed -n 's/^\(GIT_[A-Za-z0-9_]*\)=.*/\1/p'); do
  unset "$name"
done

for ref in "$base" "$head"; do
  if ! git rev-parse --verify --quiet "$ref^{commit}" >/dev/null; then
    echo "hygiene: cannot resolve $ref to a commit" >&2
    exit 2
  fi
done
if ! merge_base=$(git merge-base "$base" "$head"); then
  echo "hygiene: $base and $head share no history" >&2
  exit 2
fi

em=$(printf '\342\200\224')
en=$(printf '\342\200\223')

# Keep these in step with the shared guard hooks. The leading \b on the second
# fragment keeps a sentence about a regenerated file from matching.
msg_patterns='Co-''Authored-By|\bGener''ated with|clau''de\.ai|\banth''ropic|Clau''de'
msg_patterns="$msg_patterns"'|Chat''GPT|Open''AI|Co''pilot|Gem''ini|\bLL''M\b|AI[- ]gener''ated|AI[- ]assi''sted'
internal_paths='(^|/)\.internal/|(^|/)\.hooks/|(^|/)implementation\.md$|(^|/)CLAU''DE\.md$|(^|/)\.clau''de/'
internal_paths="$internal_paths"'|\.internal\.md$|^PRODUCT\.md$|^DESIGN\.md$'

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
found=0

# True when git treats the file as binary and its name says it should be. A NUL
# byte alone does not count, so it cannot make a source file skip the check.
is_binary_format() {
  case $1 in
    *.png | *.PNG | *.jpg | *.JPG | *.jpeg | *.JPEG | *.gif | *.GIF | *.webp | *.WEBP | *.ico | *.ICO | \
      *.woff | *.WOFF | *.woff2 | *.WOFF2 | *.ttf | *.TTF | *.otf | *.OTF | *.wasm | *.WASM | *.pdf | *.PDF) ;;
    *) return 1 ;;
  esac
  git diff --numstat "$merge_base" "$head" -- ":(literal)$1" | grep -q '^-'
}

# Files the range changes. A name git has to quote, such as one with a line
# feed in it, cannot be read back here and is reported rather than skipped.
if ! git -c core.quotePath=false diff --name-only --diff-filter=ACMR "$merge_base" "$head" >"$work/changed"; then
  echo "hygiene: git could not list the changed files" >&2
  exit 2
fi
while IFS= read -r path; do
  [ -n "$path" ] || continue
  if is_binary_format "$path"; then
    continue
  fi
  if ! git show "$head:$path" >"$work/blob" 2>/dev/null; then
    printf '%s: cannot read this path at %s\n' "$path" "$head"
    found=1
    continue
  fi
  if LC_ALL=C grep -n -a -F -e "$em" -e "$en" "$work/blob" >"$work/hits"; then
    sed "s|^|$(printf '%s' "$path" | sed 's/[|&\\]/\\&/g'):|" "$work/hits"
    found=1
  fi
done <"$work/changed"

# Commit messages in the range.
if ! git rev-list "$base..$head" >"$work/commits"; then
  echo "hygiene: git could not list the commits" >&2
  exit 2
fi
while IFS= read -r commit; do
  git log -1 --format=%B "$commit" >"$work/message"
  if LC_ALL=C grep -n -F -e "$em" -e "$en" "$work/message" >"$work/hits"; then
    sed "s|^|$commit:|" "$work/hits"
    found=1
  fi
  if grep -n -i -E "$msg_patterns" "$work/message" >"$work/hits"; then
    sed "s|^|$commit:|" "$work/hits"
    found=1
  fi
done <"$work/commits"

# Paths tracked at the head.
if ! git -c core.quotePath=false ls-tree -r --name-only "$head" >"$work/tracked"; then
  echo "hygiene: git could not list the tracked files" >&2
  exit 2
fi
if grep -E "$internal_paths" "$work/tracked" >"$work/hits"; then
  sed 's/$/: internal file tracked/' "$work/hits"
  found=1
fi

if [ "$found" -ne 0 ]; then
  echo "hygiene: failed over $base..$head" >&2
  exit 1
fi
echo "hygiene: clean over $base..$head"
