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
# With --check-order it also checks the check order blocks, the lines
# between "<!-- check-order: <name> -->" and "<!-- /check-order -->". Every
# "### <name>" operation heading under "## Operations and check orders",
# except "Views", must be followed by exactly one block of the same name, and
# no block may sit anywhere else. A block holds only numbered checks, numbered
# 1, 2, 3 in order (a letter suffix such as "9a. " follows its step). Each
# check must cite exactly one MANDATE rule ID, that rule must be defined, and
# it must be able to fail: its Error line may not start with "none". A rule
# that hands off to per-item checks says so with "- Error: per item".
# A step may not cite a rule whose precondition cites, directly or through
# rules that no step of the same order checks, a rule that another step of the
# same order checks, since the first error would then depend on how an
# implementation read it. A precondition runs from "- Precondition:" to the
# next field line, blank line or heading, whatever the indentation. Rules that
# only mention another rule do so in their Effect. Tab characters are refused
# inside rule bodies, and rule headings must run in numeric order per area.
#
# Formatting: once the fields of a rule start, a blank line may only be
# followed by a heading, so every field is contiguous and nothing can hide
# after a blank line. Two blank lines in a row are refused, every heading
# needs a blank line before it, line endings must be LF, and hyphen
# look-alikes (U+2010 to U+2015, U+2212, U+FE58, U+FE63, U+FF0D, U+00AD) are
# refused here and in README.md and the JSON files beside the spec.
#
# Everything inside fenced code blocks is ignored. An ID whose number does not
# have exactly three digits, or that runs on into letters (MANDATE-SWEEP-004a),
# is reported, never truncated. Letter suffixes on steps run a, b, c in order.
# An empty "- Error:" line is reported; a rule that adds no failure writes
# "- Error: none".
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

# Hyphen look-alikes that read as "-" but are not: U+2010 to U+2015, U+2212,
# U+FE58, U+FE63, U+FF0D and the soft hyphen U+00AD, as UTF-8 bytes.
hyphens=$(printf '\342\200\220 \342\200\221 \342\200\222 \342\200\223 \342\200\224 \342\200\225 \342\210\222 \357\271\230 \357\271\243 \357\274\215 \302\255')

# The other text files of the spec directory (README.md and the JSON tables)
# must not hold hyphen look-alikes either.
sibling_status=0
spec_dir=$(dirname "$spec")
for other in "$spec_dir"/README.md "$spec_dir"/*.json; do
  [ -f "$other" ] || continue
  for h in $hyphens; do
    if hit=$(LC_ALL=C grep -n -F -- "$h" "$other" | head -n 1); then
      echo "check-spec-ids: $(basename "$other"):${hit%%:*} has a non-ASCII hyphen; write the ASCII hyphen-minus" >&2
      sibling_status=1
      break
    fi
  done
done

set +e
LC_ALL=C awk \
  -v check_order="$check_order" \
  -v mandate_areas="$mandate_areas" \
  -v chainops_areas="$chainops_areas" \
  -v hyphens="$hyphens" '
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

# Returns the well formed rule IDs in s, separated by spaces, and reports any
# ID-like token whose number does not have exactly three digits.
function ids_in(s, where,    out, tok, digits) {
  out = ""
  while (match(s, tok_re)) {
    tok = substr(s, RSTART, RLENGTH)
    s = substr(s, RSTART + RLENGTH)
    digits = tok
    sub(/^[A-Z]+-[A-Z]+-/, "", digits)
    if (digits !~ /^[0-9][0-9][0-9]$/) {
      problem("malformed rule ID " tok " at line " where ", the number must be exactly three digits")
      continue
    }
    out = out " " tok
  }
  return out
}

BEGIN {
  tok_re = "(MANDATE|CHAINOPS)-[A-Z]+-[0-9]+[A-Za-z0-9_]*"
  ops_section = "Operations and check orders"
  section = ""
  in_order = 0
  in_fence = 0
  op = ""
  nh = split(hyphens, hy, " ")
  prev_blank = 0
}

{
  line = $0
  if (line ~ /\r$/) {
    if (!cr_seen) problem("line " NR " ends with a carriage return; line endings must be LF")
    cr_seen = 1
    sub(/\r$/, "", line)
    $0 = line
  }
  for (h = 1; h <= nh; h++) {
    if (index(line, hy[h]) > 0) {
      problem("non-ASCII hyphen at line " NR "; write the ASCII hyphen-minus")
      break
    }
  }
  is_blank = (line ~ /^[ \t]*$/)
  if (!in_fence) {
    if (is_blank && prev_blank) problem("two blank lines in a row at line " NR)
    if (!is_blank && line ~ /^#+ / && NR > 1 && !prev_blank) problem("heading at line " NR " needs a blank line before it")
    # The fields of a rule are contiguous: once they start, a blank line may
    # only be followed by another heading.
    if (current != "" && fields_started) {
      if (is_blank) pending_blank = 1
      else if (pending_blank && line !~ /^#+ /) {
        problem(current " has content after a blank line at line " NR "; the fields of a rule must be contiguous")
        pending_blank = 0
      }
    }
  }
  prev_blank = is_blank
}

# Fenced code blocks are examples, not spec text: nothing inside counts.
/^```/ { in_fence = !in_fence; next }
in_fence { next }

# Section tracking: level two headings name the section.
/^## / {
  close_rule()
  section = substr(line, 4)
  op = ""
}

# Operation headings under the operations section.
/^### / && section == ops_section {
  close_rule()
  op = substr(line, 5)
  sub(/ .*$/, "", op)
  gsub(/`/, "", op)
  if (op != "Views") {
    if (op in op_line) problem("operation " op " has two headings")
    op_line[op] = NR
  }
  next
}

# Rule headings.
/^#### / {
  close_rule()
  rest = substr(line, 6)
  if (match(rest, "^" tok_re)) {
    found = ids_in(substr(rest, RSTART, RLENGTH), NR)
    sub(/^ /, "", found)
    if (found != "") {
      if (found in defined) problem(found " is defined twice (lines " defined[found] " and " NR ")")
      else defined[found] = NR
      area_key = found
      sub(/-[0-9]+$/, "", area_key)
      seq = found
      sub(/^.*-/, "", seq)
      if ((area_key in area_last) && seq + 0 < area_last[area_key] + 0) {
        problem(found " at line " NR " comes after " area_key "-" area_last_txt[area_key] "; keep each area in numeric order")
      }
      if (!(area_key in area_last) || seq + 0 > area_last[area_key] + 0) {
        area_last[area_key] = seq
        area_last_txt[area_key] = seq
      }
      current = found
      field = ""
      fields_started = 0
      pending_blank = 0
      has_pre = has_eff = has_err = has_off = 0
    }
  }
  next
}

/^#+ / { close_rule() }

current != "" && /\t/ { problem(current " has a tab character at line " NR "; indent with spaces") }
current != "" && /^- (Precondition|Effect|Error|Off chain):/ { field = ""; fields_started = 1 }
current != "" && /^[ \t]*$/ { field = "" }
current != "" && /^- Precondition:/ { has_pre = 1; field = "pre" }
current != "" && /^- Effect:/ { has_eff = 1 }
current != "" && /^- Off chain:/ { has_off = 1 }
# The rules a precondition includes, on its own line or its indented sub-bullets.
current != "" && field == "pre" {
  found = ids_in(line, NR)
  k = split(found, got, " ")
  for (j = 1; j <= k; j++) if (got[j] != current) pre_refs[current] = pre_refs[current] " " got[j]
}
current != "" && /^- Error:/ {
  has_err = 1
  text = substr(line, 10)
  sub(/^ +/, "", text)
  sub(/ +$/, "", text)
  if (text == "") problem(current " has an empty \"- Error:\" field; write none or the error names")
  err_text[current] = text
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
  if (match(line, "^\\| *`" tok_re "`")) {
    found = ids_in(substr(line, RSTART, RLENGTH), NR)
    sub(/^ /, "", found)
    if (found != "") {
      if (found in indexed) problem(found " is listed twice in the rule index")
      indexed[found] = NR
    }
  }
  next
}

# Check order blocks.
/^<!-- check-order: [a-z_]+ -->$/ {
  if (in_order) order_problem("check order block at line " NR " opens inside another block")
  in_order = 1
  order_name = line
  sub(/^<!-- check-order: /, "", order_name)
  sub(/ -->$/, "", order_name)
  if (order_name in order_seen) order_problem("check order " order_name " appears twice")
  order_seen[order_name] = NR
  if (section != ops_section || op == "") order_problem("check order " order_name " at line " NR " is not under an operation heading")
  else if (op != order_name) order_problem("check order " order_name " at line " NR " sits under the heading for " op)
  order_lines = 0
  prev_n = 0
  prev_letter = ""
  next
}
/^<!-- \/check-order -->$/ {
  if (!in_order) order_problem("check order block closed at line " NR " without being opened")
  else if (order_lines == 0) order_problem("check order " order_name " has no numbered checks")
  in_order = 0
  next
}
in_order && /^[ \t]*$/ { next }
in_order {
  if (line !~ /^[0-9]+[a-z]?\. /) {
    order_problem(order_name ": line " NR " is not a numbered check (\"1. \" or \"9a. \")")
    next
  }
  order_lines++
  numtxt = line
  sub(/\..*$/, "", numtxt)
  letter = numtxt
  sub(/^[0-9]+/, "", letter)
  n = numtxt
  sub(/[a-z]$/, "", n)
  n = n + 0
  if (letter == "") {
    if (n != prev_n + 1) order_problem(order_name ": line " NR " is step " n ", expected step " (prev_n + 1))
    prev_letter = ""
  } else if (n != prev_n || letter != substr("abcdefghijklmnopqrstuvwxyz", index("abcdefghijklmnopqrstuvwxyz", (prev_letter == "" ? "`" : prev_letter)) + 1, 1)) {
    order_problem(order_name ": line " NR " is step " numtxt ", which does not follow step " prev_n prev_letter)
  } else {
    prev_letter = letter
  }
  if (letter == "") prev_n = n
  found = ids_in(line, NR)
  k = split(found, got, " ")
  m = 0
  first = ""
  for (j = 1; j <= k; j++) if (got[j] ~ /^MANDATE-/) { m++; if (first == "") first = got[j] }
  if (m != 1) order_problem(order_name ": line " NR " cites " m " rule IDs, needs exactly one")
  else {
    order_ref[NR] = first
    order_block[NR] = order_name
    order_step[NR] = numtxt
    block_rules[order_name] = block_rules[order_name] " " first "@" numtxt
  }
}

# Every ID cited anywhere, outside headings and the index.
{
  found = ids_in(line, NR)
  k = split(found, got, " ")
  for (j = 1; j <= k; j++) if (!(got[j] in cited)) cited[got[j]] = NR
}

# Check order problems count only with --check-order.
function order_problem(msg) {
  if (check_order) problem(msg)
}

END {
  close_rule()
  if (in_order) order_problem("check order " order_name " is never closed")

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
    ops = 0
    for (o in op_line) {
      ops++
      if (!(o in order_seen)) problem("operation " o " at line " op_line[o] " has no check order block")
    }
    for (ln in order_ref) {
      id = order_ref[ln]
      if (!(id in defined)) problem("check order line " ln " cites " id ", which is not defined")
      else if (err_text[id] ~ /^none/) problem("check order line " ln " cites " id ", which returns no error")
    }
    # A step may not cite a rule whose precondition includes, directly or
    # through rules that no step of the same order checks, a rule that another
    # step checks: the first error would depend on how an implementation read
    # it. Visited rules are tracked, so reference cycles end.
    for (ln in order_ref) {
      a = order_ref[ln]
      blk = order_block[ln]
      delete step_of
      nb = split(block_rules[blk], steps, " ")
      for (y = 1; y <= nb; y++) {
        split(steps[y], sp, "@")
        if (!(sp[1] in step_of)) step_of[sp[1]] = sp[2]
      }
      delete seen
      delete via
      top = 0
      seen[a] = 1
      nr = split(pre_refs[a], inc, " ")
      for (x = 1; x <= nr; x++) { top++; stack[top] = inc[x]; via[inc[x]] = "" }
      while (top > 0) {
        b = stack[top]
        top--
        if (b in seen) continue
        seen[b] = 1
        if ((b in step_of) && step_of[b] != order_step[ln]) {
          problem(blk ": line " ln " cites " a ", whose precondition includes " b (via[b] == "" ? "" : " (through " via[b] ")") ", which step " step_of[b] " also checks")
          continue
        }
        if (b in step_of) continue
        nr2 = split(pre_refs[b], inc2, " ")
        for (x = 1; x <= nr2; x++) {
          if (!(inc2[x] in seen)) {
            top++
            stack[top] = inc2[x]
            if (!(inc2[x] in via)) via[inc2[x]] = (via[b] == "" ? b : via[b] ", " b)
          }
        }
      }
    }
    if (problems == 0) printf "check orders: %d block(s) for %d operation(s), every numbered check cites one defined rule\n", blocks, ops
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
awk_status=$?
set -e
if [ "$awk_status" -ne 0 ]; then exit "$awk_status"; fi
exit "$sibling_status"
