#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Self test for check-spec-ids.sh. Each case writes a small spec to a temp
# directory and checks the exit code and output.
set -euo pipefail

script="$(cd "$(dirname "$0")" && pwd)/check-spec-ids.sh"
work=$(cd "$(mktemp -d)" && pwd -P)
trap 'rm -rf "$work"' EXIT

passed=0
failures=0

mandate_areas="CONFIG ROLE AMOUNT FUND ASSET DUST PAYOUT BATCH BUDGET PAYMENT DAILY RECIPIENT SPLIT APPROVAL EXPIRY PAUSE REVOKE WITHDRAW SWEEP RECOVER RECOVERYSET PRINCIPAL EVENT FEE"
chainops_areas="XCHAIN DELIVERY SUBMIT CREATE SWEEP FEE"

# Writes a minimal valid spec with one rule per area to $1. Extra text from
# $2 is appended before the rule index, and $3 is appended to the index.
valid_spec() {
  local out=$1 extra=${2:-} extra_index=${3:-} a
  {
    echo "# Spec"
    echo
    echo "## Errors"
    echo
    echo "| Code | Name | Meaning |"
    echo "| --- | --- | --- |"
    echo "| 10 | \`UNAUTHORIZED\` | Caller lacks the role |"
    echo "| 46 | \`FEE_EXCEEDS_CAP\` | Fee above the cap |"
    echo
    echo "## Rules"
    echo
    for a in $mandate_areas; do
      echo "#### MANDATE-$a-001 A $a rule"
      echo
      echo "- Precondition: something holds."
      echo "- Effect: something happens."
      echo "- Error: \`UNAUTHORIZED\`"
      echo
    done
    for a in $chainops_areas; do
      echo "#### CHAINOPS-$a-001 A $a obligation"
      echo
      echo "- Off chain: the service does something."
      echo
    done
    echo "## Operations and check orders"
    echo
    echo "### request_payout"
    echo
    echo "<!-- check-order: request_payout -->"
    echo "1. Caller is an agent (MANDATE-ROLE-001)."
    echo "2. Fee within the cap (MANDATE-FEE-001)."
    echo "<!-- /check-order -->"
    echo
    printf '%s\n' "$extra"
    echo "## Rule index"
    echo
    echo "| ID | Title |"
    echo "| --- | --- |"
    for a in $mandate_areas; do echo "| \`MANDATE-$a-001\` | A $a rule |"; done
    for a in $chainops_areas; do echo "| \`CHAINOPS-$a-001\` | A $a obligation |"; done
    printf '%s\n' "$extra_index"
  } >"$out"
}

# expect <case name> <exit code> <spec file> <flags> [text the output must contain]...
expect() {
  local name=$1 want=$2 file=$3 flags=$4
  shift 4
  local out code=0 needle
  # shellcheck disable=SC2086 # flags is a deliberate word list
  out=$("$script" $flags "$file" 2>&1) || code=$?
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

f="$work/valid.md"
valid_spec "$f"
expect "valid spec passes" 0 "$f" "" "30 rule ID(s) defined" "area MANDATE-FEE: 1 rule(s)" "area CHAINOPS-SWEEP: 1 rule(s), off chain, excluded from vector coverage"
expect "valid spec passes check order" 0 "$f" "--check-order" "check orders: 1 block(s) for 1 operation(s)"

f="$work/dup.md"
valid_spec "$f" "#### MANDATE-ROLE-001 Defined again

- Precondition: x.
- Effect: y.
- Error: none
"
expect "duplicate id" 1 "$f" "" "MANDATE-ROLE-001 is defined twice"

f="$work/missing-index.md"
valid_spec "$f" "#### MANDATE-ROLE-002 Not indexed

- Precondition: x.
- Effect: y.
- Error: none
"
expect "missing from index" 1 "$f" "" "MANDATE-ROLE-002 is defined at line" "missing from the rule index"

f="$work/index-only.md"
valid_spec "$f" "" "| \`MANDATE-ROLE-009\` | Ghost |"
expect "indexed but not defined" 1 "$f" "" "MANDATE-ROLE-009 is in the rule index but not defined"

f="$work/index-twice.md"
valid_spec "$f" "" "| \`MANDATE-ROLE-001\` | Again |"
expect "listed twice in the index" 1 "$f" "" "MANDATE-ROLE-001 is listed twice in the rule index"

f="$work/unknown-ref.md"
valid_spec "$f" "See MANDATE-PAUSE-042 for details."
expect "unknown reference" 1 "$f" "" "MANDATE-PAUSE-042 is cited at line"

f="$work/no-error-field.md"
valid_spec "$f" "#### MANDATE-ROLE-003 No error field

- Precondition: x.
- Effect: y.
" "| \`MANDATE-ROLE-003\` | No error field |"
expect "missing error field" 1 "$f" "" "MANDATE-ROLE-003 has no \"- Error:\" field"

f="$work/no-offchain.md"
valid_spec "$f" "#### CHAINOPS-FEE-002 No marker

- Something.
" "| \`CHAINOPS-FEE-002\` | No marker |"
expect "chainops rule without off chain field" 1 "$f" "" "CHAINOPS-FEE-002 has no \"- Off chain:\" field"

f="$work/unknown-error.md"
valid_spec "$f" "#### MANDATE-ROLE-004 Unknown error

- Precondition: x.
- Effect: y.
- Error: \`NOT_A_REAL_ERROR\`
" "| \`MANDATE-ROLE-004\` | Unknown error |"
expect "unknown error name" 1 "$f" "" "error NOT_A_REAL_ERROR is cited by"

f="$work/missing-area.md"
valid_spec "$f"
grep -v 'MANDATE-DUST-001' "$f" | grep -v 'A DUST rule' >"$f.tmp" && mv "$f.tmp" "$f"
expect "missing area" 1 "$f" "" "rule area MANDATE-DUST has no rule"

f="$work/unknown-area.md"
valid_spec "$f" "#### MANDATE-WIDGET-001 Not an area

- Precondition: x.
- Effect: y.
- Error: none
" "| \`MANDATE-WIDGET-001\` | Not an area |"
expect "unknown area" 1 "$f" "" "MANDATE-WIDGET-001 uses an area that is not in the area list"

f="$work/order-no-id.md"
valid_spec "$f" "### approve

<!-- check-order: approve -->
1. Caller is an approver.
<!-- /check-order -->"
expect "check order line without an id" 1 "$f" "--check-order" "approve: line" "cites 0 rule IDs"
expect "check order ignored without the flag" 0 "$f" ""

f="$work/order-two-ids.md"
valid_spec "$f" "### approve

<!-- check-order: approve -->
1. Caller is an approver (MANDATE-ROLE-001, MANDATE-PAUSE-001).
<!-- /check-order -->"
expect "check order line with two ids" 1 "$f" "--check-order" "cites 2 rule IDs"

f="$work/order-unclosed.md"
valid_spec "$f" "### approve

<!-- check-order: approve -->
1. Caller is an approver (MANDATE-ROLE-001)."
expect "check order never closed" 1 "$f" "--check-order" "check order approve is never closed"

f="$work/order-empty.md"
valid_spec "$f" "### approve

<!-- check-order: approve -->
<!-- /check-order -->"
expect "check order block without checks" 1 "$f" "--check-order" "check order approve has no numbered checks"

f="$work/order-twice.md"
valid_spec "$f" "<!-- check-order: request_payout -->
1. Again (MANDATE-ROLE-001).
<!-- /check-order -->"
expect "check order name used twice" 1 "$f" "--check-order" "check order request_payout appears twice"

f="$work/dup-code.md"
valid_spec "$f"
# shellcheck disable=SC2016 # the backticks are literal markdown
sed 's/^| 46 | `FEE_EXCEEDS_CAP`/| 10 | `FEE_EXCEEDS_CAP`/' "$f" >"$f.tmp" && mv "$f.tmp" "$f"
expect "duplicate error code" 1 "$f" "" "error code 10 is listed twice"

f="$work/dup-name.md"
valid_spec "$f"
# shellcheck disable=SC2016 # the backticks are literal markdown
sed 's/^| 46 | `FEE_EXCEEDS_CAP`/| 46 | `UNAUTHORIZED`/' "$f" >"$f.tmp" && mv "$f.tmp" "$f"
expect "duplicate error name" 1 "$f" "" "error name UNAUTHORIZED is listed twice"

f="$work/order-no-error.md"
valid_spec "$f" "#### MANDATE-PAUSE-002 Never fails

- Precondition: x.
- Effect: y.
- Error: none

### pause

<!-- check-order: pause -->
1. Pause (MANDATE-PAUSE-002).
<!-- /check-order -->" "| \`MANDATE-PAUSE-002\` | Never fails |"
expect "check cites a rule that returns no error" 1 "$f" "--check-order" "cites MANDATE-PAUSE-002, which returns no error"

f="$work/order-per-item.md"
valid_spec "$f" "#### MANDATE-BATCH-002 Per item

- Precondition: x.
- Effect: y.
- Error: per item (items report their own codes)

### batch

<!-- check-order: batch -->
1. Each item (MANDATE-BATCH-002).
<!-- /check-order -->" "| \`MANDATE-BATCH-002\` | Per item |"
expect "check cites a per-item rule" 0 "$f" "--check-order" "check orders: 2 block(s) for 2 operation(s)"

f="$work/op-without-block.md"
valid_spec "$f" "### sweep

Anyone may call it."
expect "operation heading without a block" 1 "$f" "--check-order" "operation sweep at line" "has no check order block"
expect "operation heading without a block passes without the flag" 0 "$f" ""

f="$work/block-renamed.md"
valid_spec "$f" "### sweep

<!-- check-order: sweeep -->
1. Not active (MANDATE-SWEEP-001).
<!-- /check-order -->"
expect "block named after another operation" 1 "$f" "--check-order" "check order sweeep at line" "sits under the heading for sweep" "operation sweep at line"

f="$work/block-orphan.md"
valid_spec "$f" "## Notes

<!-- check-order: approve -->
1. Caller is an approver (MANDATE-ROLE-001).
<!-- /check-order -->
"
expect "block outside the operations section" 1 "$f" "--check-order" "check order approve at line" "is not under an operation heading"

f="$work/views-excluded.md"
valid_spec "$f" "### Views

The views change nothing."
expect "views heading needs no block" 0 "$f" "--check-order" "check orders: 1 block(s) for 1 operation(s)"

f="$work/empty-error.md"
valid_spec "$f" "#### MANDATE-ROLE-005 Empty error

- Precondition: x.
- Effect: y.
- Error:
" "| \`MANDATE-ROLE-005\` | Empty error |"
expect "empty error field" 1 "$f" "" "MANDATE-ROLE-005 has an empty \"- Error:\" field"

f="$work/order-bullet.md"
valid_spec "$f" "### approve

<!-- check-order: approve -->
1. Caller is an approver (MANDATE-ROLE-001).
- Also the payout exists.
<!-- /check-order -->"
expect "bullet inside a check order" 1 "$f" "--check-order" "is not a numbered check"

f="$work/order-paren.md"
valid_spec "$f" "### approve

<!-- check-order: approve -->
1. Caller is an approver (MANDATE-ROLE-001).
2) The payout exists.
<!-- /check-order -->"
expect "parenthesis numbering inside a check order" 1 "$f" "--check-order" "is not a numbered check"

f="$work/order-dup-step.md"
valid_spec "$f" "### approve

<!-- check-order: approve -->
1. Caller is an approver (MANDATE-ROLE-001).
1. Not paused (MANDATE-PAUSE-001).
<!-- /check-order -->"
expect "duplicate step number" 1 "$f" "--check-order" "is step 1, expected step 2"

f="$work/order-skip-step.md"
valid_spec "$f" "### approve

<!-- check-order: approve -->
1. Caller is an approver (MANDATE-ROLE-001).
3. Not paused (MANDATE-PAUSE-001).
<!-- /check-order -->"
expect "skipped step number" 1 "$f" "--check-order" "is step 3, expected step 2"

f="$work/order-letter.md"
valid_spec "$f" "### approve

<!-- check-order: approve -->
1. Caller is an approver (MANDATE-ROLE-001).
1a. Not paused (MANDATE-PAUSE-001).
2. Fee within the cap (MANDATE-FEE-001).
<!-- /check-order -->"
expect "letter suffix follows its step" 0 "$f" "--check-order"

f="$work/four-digit.md"
valid_spec "$f" "#### MANDATE-ROLE-0010 Four digits

- Precondition: x.
- Effect: y.
- Error: none
"
expect "four digit id is rejected, not truncated" 1 "$f" "" "malformed rule ID MANDATE-ROLE-0010"

f="$work/fenced-field.md"
valid_spec "$f" "#### MANDATE-ROLE-006 Field in a fence

- Precondition: x.
- Effect: y.

\`\`\`
- Error: \`UNAUTHORIZED\`
\`\`\`
" "| \`MANDATE-ROLE-006\` | Field in a fence |"
expect "field inside a fenced code block does not count" 1 "$f" "" "MANDATE-ROLE-006 has no \"- Error:\" field"

expect "unreadable file" 2 "$work/does-not-exist.md" ""
expect "unknown option" 2 "$work/valid.md" "--frobnicate"

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
