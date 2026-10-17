#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Checks the rule IDs in spec/MANDATE.md.
#
# A rule is a level four heading that starts with its ID:
#
#   #### MANDATE-<AREA>-<NNN> Title
#   #### CHAINOPS-<AREA>-<NNN> Title
#
# Every MANDATE rule must carry the three fields "- Precondition:",
# "- Effect:" and "- Error:" before the next heading, and every error name it
# cites in backticks on its Error line must be listed in the error table (a
# "## Errors" section whose rows read "| <code> | `NAME` | ..."). Every
# CHAINOPS rule must carry an "- Off chain:" field; those rules bind services,
# not the contracts, and are reported as excluded from vector coverage.
#
# The check fails when the error table lists a code or a name twice, when an
# ID is defined twice, when an ID is defined but not in the rule index (a "## Rule index" section with rows "| `ID` | ... |"), when
# the index lists an ID twice or lists one that is not defined, when any ID
# cited anywhere is not defined, when a field is missing, when an error name is
# unknown, or when a required rule area has no rule.
#
# With --check-order it also checks every check order block, the lines
# between "<!-- check-order: <name> -->" and "<!-- /check-order -->". Each
# numbered line ("1. ", "9a. ") must cite exactly one MANDATE rule ID, that
# rule must be defined, and it must be able to fail: its Error line may not
# start with "none". A rule that hands off to per-item checks says so with
# "- Error: per item".
#
# Usage: scripts/check-spec-ids.sh [--check-order] [path to MANDATE.md]
# Exit codes: 0 all checks pass, 1 one or more problems named on stderr,
# 2 bad usage or an unreadable file.
set -euo pipefail

check_order=0
spec=""
for arg in "$@"; do
  case "$arg" in
    --check-order) check_order=1 ;;
    -*)
      echo "check-spec-ids: unknown option $arg" >&2
      exit 2
      ;;
    *)
      if [ -n "$spec" ]; then
        echo "check-spec-ids: more than one file given" >&2
        exit 2
      fi
      spec=$arg
      ;;
  esac
done
spec=${spec:-"$(cd "$(dirname "$0")/.." && pwd)/spec/MANDATE.md"}

if [ ! -r "$spec" ]; then
  echo "check-spec-ids: cannot read $spec" >&2
  exit 2
fi

# The areas the spec must cover. Keep in step with the spec's own list.
mandate_areas="CONFIG ROLE AMOUNT FUND ASSET DUST PAYOUT BATCH BUDGET PAYMENT DAILY RECIPIENT SPLIT APPROVAL EXPIRY PAUSE REVOKE WITHDRAW SWEEP RECOVER RECOVERYSET PRINCIPAL EVENT FEE"
chainops_areas="XCHAIN DELIVERY SUBMIT CREATE SWEEP FEE"

LC_ALL=C awk \
  -v check_order="$check_order" \
  -v mandate_areas="$mandate_areas" \
  -v chainops_areas="$chainops_areas" '
function problem(msg) {
  print "check-spec-ids: " msg > "/dev/stderr"
  problems++
}

# Closes the rule that was open, checking its fields.
function close_rule() {
  if (current == "") return
  if (current ~ /^MANDATE-/) {
    if (!has_pre) problem(current " has no \"- Precondition:\" field")
    if (!has_eff) problem(current " has no \"- Effect:\" field")
    if (!has_err) problem(current " has no \"- Error:\" field")
  } else if (!has_off) {
    problem(current " has no \"- Off chain:\" field")
  }
  current = ""
}

BEGIN {
  id_re = "(MANDATE|CHAINOPS)-[A-Z]+-[0-9][0-9][0-9]"
  section = ""
  in_order = 0
}

{
  line = $0
}

# Section tracking: level two headings name the section.
/^## / {
  close_rule()
  section = substr(line, 4)
}

# Rule headings.
/^#### / {
  close_rule()
  rest = substr(line, 6)
  if (match(rest, "^" id_re)) {
    id = substr(rest, RSTART, RLENGTH)
    if (id in defined) problem(id " is defined twice (lines " defined[id] " and " NR ")")
    else defined[id] = NR
    current = id
    has_pre = has_eff = has_err = has_off = 0
  }
  next
}

/^#+ / { close_rule() }

current != "" && /^- Precondition:/ { has_pre = 1 }
current != "" && /^- Effect:/ { has_eff = 1 }
current != "" && /^- Off chain:/ { has_off = 1 }
current != "" && /^- Error:/ {
  has_err = 1
  err_text[current] = substr(line, 10)
  rest = line
  while (match(rest, /`[A-Z][A-Z0-9_]*`/)) {
    name = substr(rest, RSTART + 1, RLENGTH - 2)
    cited_error[name] = cited_error[name] " " current
    rest = substr(rest, RSTART + RLENGTH)
  }
}

# Error table rows: | 10 | `UNAUTHORIZED` | ...
section == "Errors" && /^\| *[0-9]+ *\| *`[A-Z][A-Z0-9_]*`/ {
  code = line
  sub(/^\| */, "", code)
  sub(/ *\|.*$/, "", code)
  if (code in error_code) problem("error code " code " is listed twice (lines " error_code[code] " and " NR ")")
  else error_code[code] = NR
  if (match(line, /`[A-Z][A-Z0-9_]*`/)) {
    name = substr(line, RSTART + 1, RLENGTH - 2)
    if (name in known_error) problem("error name " name " is listed twice")
    known_error[name] = 1
  }
}

# Rule index rows: | `MANDATE-ROLE-001` | ...
section == "Rule index" && /^\| *`/ {
  if (match(line, "^\\| *`" id_re "`")) {
    s = substr(line, RSTART, RLENGTH)
    match(s, id_re)
    id = substr(s, RSTART, RLENGTH)
    if (id in indexed) problem(id " is listed twice in the rule index")
    indexed[id] = NR
  }
  next
}

# Check order blocks.
/^<!-- check-order: [a-z_]+ -->$/ {
  if (in_order) problem("check order block at line " NR " opens inside another block")
  in_order = 1
  order_name = line
  sub(/^<!-- check-order: /, "", order_name)
  sub(/ -->$/, "", order_name)
  if (order_name in order_seen) problem("check order " order_name " appears twice")
  order_seen[order_name] = 1
  order_lines = 0
  next
}
/^<!-- \/check-order -->$/ {
  if (!in_order) problem("check order block closed at line " NR " without being opened")
  else if (order_lines == 0) problem("check order " order_name " has no numbered checks")
  in_order = 0
  next
}
in_order && /^[0-9]+[a-z]?\. / {
  order_lines++
  n = 0
  rest = line
  first = ""
  while (match(rest, "MANDATE-[A-Z]+-[0-9][0-9][0-9]")) {
    n++
    if (first == "") first = substr(rest, RSTART, RLENGTH)
    rest = substr(rest, RSTART + RLENGTH)
  }
  if (n != 1) order_bad[NR] = order_name ": line " NR " cites " n " rule IDs, needs exactly one"
  else order_ref[NR] = first
}

# Every ID cited anywhere, outside headings and the index.
{
  rest = line
  while (match(rest, id_re)) {
    id = substr(rest, RSTART, RLENGTH)
    if (!(id in cited)) cited[id] = NR
    rest = substr(rest, RSTART + RLENGTH)
  }
}

END {
  close_rule()
  if (in_order) problem("check order " order_name " is never closed")

  for (id in defined) if (!(id in indexed)) problem(id " is defined at line " defined[id] " but missing from the rule index")
  for (id in indexed) if (!(id in defined)) problem(id " is in the rule index but not defined")
  for (id in cited) if (!(id in defined)) problem(id " is cited at line " cited[id] " but not defined")

  for (name in cited_error) if (!(name in known_error)) problem("error " name " is cited by" cited_error[name] " but not in the error table")

  split(mandate_areas, ma, " ")
  for (i = 1; i in ma; i++) count_m[ma[i]] = 0
  split(chainops_areas, ca, " ")
  for (i = 1; i in ca; i++) count_c[ca[i]] = 0
  for (id in defined) {
    split(id, parts, "-")
    if (parts[1] == "MANDATE") {
      if (!(parts[2] in count_m)) problem(id " uses an area that is not in the area list")
      else count_m[parts[2]]++
    } else {
      if (!(parts[2] in count_c)) problem(id " uses an area that is not in the area list")
      else count_c[parts[2]]++
    }
  }
  for (i = 1; i in ma; i++) {
    a = ma[i]
    if (count_m[a] == 0) problem("rule area MANDATE-" a " has no rule")
    else {
      errs = ""
      for (name in cited_error) if (index(cited_error[name], " MANDATE-" a "-") > 0) errs = errs " " name
      printf "area MANDATE-%s: %d rule(s); errors:%s\n", a, count_m[a], (errs == "" ? " none" : errs)
    }
  }
  for (i = 1; i in ca; i++) {
    a = ca[i]
    if (count_c[a] == 0) problem("rule area CHAINOPS-" a " has no rule")
    else printf "area CHAINOPS-%s: %d rule(s), off chain, excluded from vector coverage\n", a, count_c[a]
  }

  if (check_order) {
    blocks = 0
    for (b in order_seen) blocks++
    if (blocks == 0) problem("no check order blocks found")
    for (ln in order_bad) problem(order_bad[ln])
    for (ln in order_ref) {
      id = order_ref[ln]
      if (!(id in defined)) problem("check order line " ln " cites " id ", which is not defined")
      else if (err_text[id] ~ /^none/) problem("check order line " ln " cites " id ", which returns no error")
    }
    if (problems == 0) printf "check orders: %d block(s), every numbered check cites one defined rule\n", blocks
  }

  total = 0
  for (id in defined) total++
  if (problems > 0) {
    printf "check-spec-ids: %d problem(s)\n", problems > "/dev/stderr"
    exit 1
  }
  printf "%d rule ID(s) defined, indexed and unique\n", total
}
' "$spec"
