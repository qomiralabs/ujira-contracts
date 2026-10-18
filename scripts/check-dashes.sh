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
# tree, so untracked files are not checked. Binary files are skipped. Each hit
# is printed as path:line:text.
#
# Fails closed: if git cannot find the repository or search it, the check
# fails rather than passing.
#
# Usage: scripts/check-dashes.sh
# Exit codes: 0 no dashes, 1 dashes found, 2 the repository could not be
# searched.
set -euo pipefail

# An inherited repo override would make git look at some other repository.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE

if ! top=$(git rev-parse --show-toplevel 2>/dev/null) || [ -z "$top" ]; then
  echo "check-dashes: could not find the git repository" >&2
  exit 2
fi

# The two characters, written as raw UTF-8 bytes so this works with BSD and
# GNU tools.
em=$(printf '\xe2\x80\x94')
en=$(printf '\xe2\x80\x93')

# git grep exits 0 on a match, 1 on none and anything else on an error. -I
# skips binary files; core.quotePath=false prints names as they are.
code=0
LC_ALL=C git -C "$top" -c core.quotePath=false grep --cached -I -n -F -e "$em" -e "$en" -- . || code=$?

case "$code" in
  0)
    echo "Replace em and en dashes with a comma, a colon, parentheses or the word \"to\"." >&2
    exit 1
    ;;
  1)
    echo "No em or en dashes found."
    ;;
  *)
    echo "check-dashes: git could not search the repository (exit $code)" >&2
    exit 2
    ;;
esac
