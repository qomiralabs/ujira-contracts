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
    if [ -n "$extra" ]; then
      printf '%s\n' "$extra"
      case "$extra" in
        *"
") ;;
        *) echo ;;
      esac
    fi
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
# The checker reads every file git tracks in the spec's directory, so each
# case runs in a git repository of its own: a spec written straight into
# $work is copied into a new directory first, and a case directory that is
# not yet a repository is initialised and every file in it added. Cases that
# need sibling files or untracked files build their own directory.
case_count=0

# Makes $1 a git repository with every file in it added, unless it already is.
make_repo() {
  local dir=$1
  [ -e "$dir/.git" ] && return
  git -C "$dir" -c init.defaultBranch=main init -q
  git -C "$dir" add -A
}

expect() {
  local name=$1 want=$2 file=$3 flags=$4
  shift 4
  local out code=0 needle
  if [ "$(dirname "$file")" = "$work" ] && [ -e "$file" ]; then
    case_count=$((case_count + 1))
    mkdir -p "$work/cases/$case_count"
    cp "$file" "$work/cases/$case_count/"
    file="$work/cases/$case_count/$(basename "$file")"
  fi
  if [ -e "$file" ] && [ -z "${no_repo:-}" ]; then make_repo "$(dirname "$file")"; fi
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

f="$work/order-overlap.md"
valid_spec "$f" "#### MANDATE-CONFIG-002 Domains

- Precondition: a field breaks its domain:
  - the threshold as in MANDATE-FEE-001.
- Effect: refused.
- Error: \`UNAUTHORIZED\`

### create

<!-- check-order: create -->
1. Domains (MANDATE-CONFIG-002).
2. Fee within the cap (MANDATE-FEE-001).
<!-- /check-order -->" "| \`MANDATE-CONFIG-002\` | Domains |"
expect "a check whose rule includes another step's rule" 1 "$f" "--check-order" "create: line" "cites MANDATE-CONFIG-002, whose precondition includes MANDATE-FEE-001, which step 2 also checks"

f="$work/order-mention-in-effect.md"
valid_spec "$f" "#### MANDATE-CONFIG-002 Domains

- Precondition: a field breaks its domain.
- Effect: refused; the fee is MANDATE-FEE-001.
- Error: \`UNAUTHORIZED\`

### create

<!-- check-order: create -->
1. Domains (MANDATE-CONFIG-002).
2. Fee within the cap (MANDATE-FEE-001).
<!-- /check-order -->" "| \`MANDATE-CONFIG-002\` | Domains |"
expect "a rule that only mentions another in its effect" 0 "$f" "--check-order"

f="$work/order-letter-gap.md"
valid_spec "$f" "### approve

<!-- check-order: approve -->
1. Caller is an approver (MANDATE-ROLE-001).
1b. Not paused (MANDATE-PAUSE-001).
<!-- /check-order -->"
expect "letter suffix out of sequence" 1 "$f" "--check-order" "is step 1b, which does not follow step 1"

f="$work/heading-letter.md"
valid_spec "$f" "#### MANDATE-SWEEP-004a Trailing letter

- Precondition: x.
- Effect: y.
- Error: none
"
expect "rule id with a trailing letter is rejected" 1 "$f" "" "malformed rule ID MANDATE-SWEEP-004a"

f="$work/order-overlap-tab.md"
valid_spec "$f" "#### MANDATE-CONFIG-002 Domains

- Precondition: a field breaks its domain:
	- the threshold as in MANDATE-FEE-001.
- Effect: refused.
- Error: \`UNAUTHORIZED\`

### create

<!-- check-order: create -->
1. Domains (MANDATE-CONFIG-002).
2. Fee within the cap (MANDATE-FEE-001).
<!-- /check-order -->" "| \`MANDATE-CONFIG-002\` | Domains |"
expect "tab inside a rule body" 1 "$f" "--check-order" "order-overlap-tab.md:" ":1 has U+0009 (a control character)" "whose precondition includes MANDATE-FEE-001"

f="$work/order-overlap-lazy.md"
valid_spec "$f" "#### MANDATE-CONFIG-002 Domains

- Precondition: a field breaks its domain,
including the threshold as in MANDATE-FEE-001.
- Effect: refused.
- Error: \`UNAUTHORIZED\`

### create

<!-- check-order: create -->
1. Domains (MANDATE-CONFIG-002).
2. Fee within the cap (MANDATE-FEE-001).
<!-- /check-order -->" "| \`MANDATE-CONFIG-002\` | Domains |"
expect "lazy continuation of a precondition" 1 "$f" "--check-order" "cites MANDATE-CONFIG-002, whose precondition includes MANDATE-FEE-001, which step 2 also checks"

f="$work/order-overlap-transitive.md"
valid_spec "$f" "#### MANDATE-CONFIG-002 Domains

- Precondition: a field breaks MANDATE-CONFIG-003.
- Effect: refused.
- Error: \`UNAUTHORIZED\`

#### MANDATE-CONFIG-003 Inner

- Precondition: the fee as in MANDATE-FEE-001.
- Effect: refused.
- Error: \`UNAUTHORIZED\`

### create

<!-- check-order: create -->
1. Domains (MANDATE-CONFIG-002).
2. Fee within the cap (MANDATE-FEE-001).
<!-- /check-order -->" "| \`MANDATE-CONFIG-002\` | Domains |
| \`MANDATE-CONFIG-003\` | Inner |"
expect "overlap through a rule no step checks" 1 "$f" "--check-order" "cites MANDATE-CONFIG-002, whose precondition includes MANDATE-FEE-001 (through MANDATE-CONFIG-003), which step 2 also checks"

f="$work/order-overlap-cycle.md"
valid_spec "$f" "#### MANDATE-CONFIG-002 Domains

- Precondition: a field breaks MANDATE-CONFIG-003.
- Effect: refused.
- Error: \`UNAUTHORIZED\`

#### MANDATE-CONFIG-003 Inner

- Precondition: a field breaks MANDATE-CONFIG-002.
- Effect: refused.
- Error: \`UNAUTHORIZED\`

### create

<!-- check-order: create -->
1. Domains (MANDATE-CONFIG-002).
<!-- /check-order -->" "| \`MANDATE-CONFIG-002\` | Domains |
| \`MANDATE-CONFIG-003\` | Inner |"
expect "reference cycle terminates" 0 "$f" "--check-order"

f="$work/heading-order.md"
valid_spec "$f" "#### MANDATE-ROLE-003 Third

- Precondition: x.
- Effect: y.
- Error: none

#### MANDATE-ROLE-002 Second

- Precondition: x.
- Effect: y.
- Error: none
" "| \`MANDATE-ROLE-003\` | Third |
| \`MANDATE-ROLE-002\` | Second |"
expect "rule headings out of numeric order" 1 "$f" "" "MANDATE-ROLE-002 at line" "comes after MANDATE-ROLE-003"

f="$work/blank-then-indented.md"
valid_spec "$f" "#### MANDATE-CONFIG-002 Domains

- Precondition: a field breaks its domain.

  It also covers the fee as in MANDATE-FEE-001.
- Effect: refused.
- Error: \`UNAUTHORIZED\`

### create

<!-- check-order: create -->
1. Domains (MANDATE-CONFIG-002).
2. Fee within the cap (MANDATE-FEE-001).
<!-- /check-order -->" "| \`MANDATE-CONFIG-002\` | Domains |"
expect "blank line then indented content in a rule" 1 "$f" "--check-order" "MANDATE-CONFIG-002 has content after a blank line at line" "must be contiguous"

f="$work/blank-then-paragraph.md"
valid_spec "$f" "#### MANDATE-ROLE-005 Paragraph after the fields

- Precondition: x.
- Effect: y.
- Error: none

A stray paragraph inside the rule.
" "| \`MANDATE-ROLE-005\` | Paragraph after the fields |"
expect "blank line then a paragraph in a rule" 1 "$f" "" "MANDATE-ROLE-005 has content after a blank line at line"

f="$work/nb-hyphen.md"
valid_spec "$f" "See MANDATE$(printf '\342\200\221')SWEEP$(printf '\342\200\221')001 for details."
expect "non-ascii hyphen" 1 "$f" "" "nb-hyphen.md:" "has U+2011 NON-BREAKING HYPHEN"

f="$work/crlf.md"
valid_spec "$f"
sed 's/$/'"$(printf '\r')"'/' "$f" >"$f.tmp" && mv "$f.tmp" "$f"
expect "crlf line endings" 1 "$f" "" "crlf.md:1 ends with a carriage return; line endings must be LF"

f="$work/double-blank.md"
valid_spec "$f" "Some text.


More text."
expect "two blank lines in a row" 1 "$f" "" "two blank lines in a row at line"

f="$work/heading-no-blank.md"
valid_spec "$f" "Some text.
### approve"
expect "heading not preceded by a blank line" 1 "$f" "" "heading at line" "needs a blank line before it"

mkdir -p "$work/sibling"
f="$work/sibling/MANDATE.md"
valid_spec "$f"
printf 'A README with a non breaking hyphen: a%sb\n' "$(printf '\342\200\221')" >"$work/sibling/README.md"
expect "non-ascii hyphen in a sibling spec file" 1 "$f" "" "README.md:1:39 has U+2011 NON-BREAKING HYPHEN"

# Character set: spec files hold printable ASCII and LF only, plus U+2016 in
# MANDATE.md. Each case builds its own spec directory.
charset_case() {
  local dir=$1 extra=$2
  mkdir -p "$work/$dir"
  valid_spec "$work/$dir/MANDATE.md" "$extra"
}

charset_case bare-cr "### approve

<!-- check-order: approve -->
1. Caller is an approver (MANDATE-ROLE-001).$(printf '\r')2. Hidden step (MANDATE-FEE-001).
<!-- /check-order -->"
expect "bare carriage return hides a step" 1 "$work/bare-cr/MANDATE.md" "--check-order" "has a bare carriage return (U+000D); line endings must be LF"

charset_case cyrillic-m "See $(printf '\320\234')ANDATE-ROLE-001 for details."
expect "cyrillic letter in an id" 1 "$work/cyrillic-m/MANDATE.md" "" "MANDATE.md:" ":5 has U+041C CYRILLIC CAPITAL LETTER EM"

charset_case hyphen-bullet "See MANDATE$(printf '\342\201\203')ROLE$(printf '\342\201\203')001."
expect "hyphen bullet" 1 "$work/hyphen-bullet/MANDATE.md" "" "has U+2043 HYPHEN BULLET"

charset_case vertical-dash "See MANDATE$(printf '\357\270\262')ROLE-001."
expect "vertical en dash form" 1 "$work/vertical-dash/MANDATE.md" "" "has U+FE32 PRESENTATION FORM FOR VERTICAL EN DASH"

charset_case prolonged-mark "See MANDATE$(printf '\343\203\274')ROLE-001."
expect "prolonged sound mark" 1 "$work/prolonged-mark/MANDATE.md" "" "has U+30FC KATAKANA-HIRAGANA PROLONGED SOUND MARK"

charset_case nbsp "A no break$(printf '\302\240')space."
expect "no break space" 1 "$work/nbsp/MANDATE.md" "" "has U+00A0 NO-BREAK SPACE"

charset_case subfolder ""
mkdir -p "$work/subfolder/vectors/fee"
printf '{ "name": "caf%s" }\n' "$(printf '\303\251')" >"$work/subfolder/vectors/fee/cap.json"
expect "non-ascii character in a subfolder file" 1 "$work/subfolder/MANDATE.md" "" "vectors/fee/cap.json:1:15 has U+00E9 LATIN SMALL LETTER E WITH ACUTE"

charset_case allowed "Concatenation is written a $(printf '\342\200\226') b."
expect "allowlisted double vertical line in MANDATE.md passes" 0 "$work/allowed/MANDATE.md" "--check-order" "30 rule ID(s) defined"

charset_case allowed-encoding ""
printf 'fields = a %s b\n' "$(printf '\342\200\226')" >"$work/allowed-encoding/ENCODING.md"
expect "allowlisted double vertical line in ENCODING.md passes" 0 "$work/allowed-encoding/MANDATE.md" "" "30 rule ID(s) defined"

charset_case allowed-elsewhere ""
printf 'a %s b\n' "$(printf '\342\200\226')" >"$work/allowed-elsewhere/README.md"
expect "double vertical line outside MANDATE.md fails" 1 "$work/allowed-elsewhere/MANDATE.md" "" "README.md:1:3 has U+2016 DOUBLE VERTICAL LINE"

charset_case control-char "A bell$(printf '\007') here."
expect "other control character" 1 "$work/control-char/MANDATE.md" "" "has U+0007 (a control character)"

# Tracked entries: only regular files, and only ASCII paths.
charset_case dir-symlink ""
mkdir -p "$work/dir-symlink-target"
printf 'caf%s\n' "$(printf '\303\251')" >"$work/dir-symlink-target/hidden.json"
ln -s ../dir-symlink-target "$work/dir-symlink/vectors"
expect "directory symlink" 1 "$work/dir-symlink/MANDATE.md" "" "vectors is a symbolic link (mode 120000); spec files must be regular files"

charset_case dot-symlink ""
ln -s . "$work/dot-symlink/loop"
expect "symlink to the spec directory itself" 1 "$work/dot-symlink/MANDATE.md" "" "loop is a symbolic link (mode 120000)"

charset_case file-symlink ""
ln -s MANDATE.md "$work/file-symlink/README.md"
expect "file symlink" 1 "$work/file-symlink/MANDATE.md" "" "README.md is a symbolic link (mode 120000)"

charset_case greek-name ""
cp "$work/greek-name/MANDATE.md" "$work/greek-name/$(printf '\316\234')ANDATE.md"
expect "non-ascii file name" 1 "$work/greek-name/MANDATE.md" "" "path \\xce\\x9cANDATE.md has byte 0xCE at position 1; spec paths hold printable ASCII only"

charset_case control-name ""
printf 'x\n' >"$work/control-name/a$(printf '\001')b.md"
expect "control character in a file name" 1 "$work/control-name/MANDATE.md" "" "path a\\x01b.md has byte 0x01 at position 2"

charset_case untracked-ds-store ""
make_repo "$work/untracked-ds-store"
printf '\000\001Bud1\377' >"$work/untracked-ds-store/.DS_Store"
expect "untracked .DS_Store is ignored" 0 "$work/untracked-ds-store/MANDATE.md" "--check-order" "30 rule ID(s) defined"

charset_case untracked-spec ""
make_repo "$work/untracked-spec"
git -C "$work/untracked-spec" rm -q --cached MANDATE.md
expect "spec file not tracked" 1 "$work/untracked-spec/MANDATE.md" "" "MANDATE.md is not tracked by git"

# Fail closed without git or outside a repository.
charset_case no-repo ""
no_repo=1 expect "not a git repository" 2 "$work/no-repo/MANDATE.md" "" "is not in a git repository"

charset_case no-git ""
mkdir -p "$work/bin-without-git"
for tool in bash env dirname awk; do ln -s "$(command -v "$tool")" "$work/bin-without-git/$tool"; done
ln -s "$(python3 -c 'import sys; print(sys.executable)')" "$work/bin-without-git/python3"
make_repo "$work/no-git"
code=0
out=$(PATH="$work/bin-without-git" "$script" "$work/no-git/MANDATE.md" 2>&1) || code=$?
if [ "$code" -eq 2 ] && printf '%s\n' "$out" | grep -qF "git is not available"; then
  echo "ok: git not available"
  passed=$((passed + 1))
else
  echo "FAIL: git not available: exit $code, want 2"
  printf '%s\n' "$out" | sed 's/^/    /'
  failures=$((failures + 1))
fi

# Content comes from the git object store, and nothing may convert it on the
# way to the working tree.
charset_case utf7 ""
git -C "$work/utf7" -c init.defaultBranch=main init -q
printf 'MANDATE.md working-tree-encoding=UTF-7\n' >"$work/utf7/.git/info/attributes"
printf 'See +BBw-ANDATE-ROLE-001.\n' >>"$work/utf7/MANDATE.md"
git -C "$work/utf7" add -A
expect "utf-7 working tree encoding" 1 "$work/utf7/MANDATE.md" "" "MANDATE.md has attribute working-tree-encoding=UTF-7" "has U+041C CYRILLIC CAPITAL LETTER EM"

charset_case clean-filter ""
printf 'readme\n' >"$work/clean-filter/README.md"
git -C "$work/clean-filter" -c init.defaultBranch=main init -q
git -C "$work/clean-filter" config filter.sneaky.clean cat
git -C "$work/clean-filter" config filter.sneaky.smudge cat
printf 'README.md filter=sneaky\n' >"$work/clean-filter/.git/info/attributes"
git -C "$work/clean-filter" add -A
expect "clean or smudge filter" 1 "$work/clean-filter/MANDATE.md" "" "README.md has attribute filter=sneaky"

charset_case eol-crlf ""
git -C "$work/eol-crlf" -c init.defaultBranch=main init -q
printf '*.md eol=crlf\n' >"$work/eol-crlf/.git/info/attributes"
git -C "$work/eol-crlf" add -A
expect "eol=crlf attribute" 1 "$work/eol-crlf/MANDATE.md" "" "MANDATE.md has attribute eol=crlf"

charset_case ident ""
git -C "$work/ident" -c init.defaultBranch=main init -q
printf 'MANDATE.md ident\n' >"$work/ident/.git/info/attributes"
git -C "$work/ident" add -A
expect "ident attribute" 1 "$work/ident/MANDATE.md" "" "MANDATE.md has attribute ident=set"

charset_case tracked-attributes ""
printf 'vectors/*.json -text\n' >"$work/tracked-attributes/.gitattributes"
expect "tracked .gitattributes in the spec directory" 1 "$work/tracked-attributes/MANDATE.md" "" ".gitattributes is a tracked attributes file inside the spec directory"

charset_case worktree-differs ""
make_repo "$work/worktree-differs"
printf 'An unstaged line.\n' >>"$work/worktree-differs/MANDATE.md"
expect "working tree differs from the blob" 1 "$work/worktree-differs/MANDATE.md" "" "MANDATE.md in the working tree differs from the staged blob"

charset_case blob-only ""
make_repo "$work/blob-only"
cp "$work/blob-only/MANDATE.md" "$work/blob-only-clean.md"
printf 'See %sANDATE-ROLE-001.\n' "$(printf '\320\234')" >>"$work/blob-only/MANDATE.md"
git -C "$work/blob-only" add MANDATE.md
cp "$work/blob-only-clean.md" "$work/blob-only/MANDATE.md"
expect "look-alike only in the blob" 1 "$work/blob-only/MANDATE.md" "" "has U+041C CYRILLIC CAPITAL LETTER EM" "differs from the staged blob"

charset_case plain-attributes ""
git -C "$work/plain-attributes" -c init.defaultBranch=main init -q
printf '*.md -text\n' >"$work/plain-attributes/.git/info/attributes"
git -C "$work/plain-attributes" add -A
expect "attributes that convert nothing pass" 0 "$work/plain-attributes/MANDATE.md" "--check-order" "30 rule ID(s) defined"

# Inherited git variables must not point the check at another repository.
charset_case inherited-git-dir "See $(printf '\320\234')ANDATE-ROLE-001."
make_repo "$work/inherited-git-dir"
charset_case other-repo ""
make_repo "$work/other-repo"
git -C "$work/other-repo" mv MANDATE.md OTHER.md
code=0
out=$(GIT_DIR="$work/other-repo/.git" GIT_WORK_TREE="$work/other-repo" GIT_INDEX_FILE="$work/other-repo/.git/index" "$script" "$work/inherited-git-dir/MANDATE.md" 2>&1) || code=$?
if [ "$code" -eq 1 ] && printf '%s\n' "$out" | grep -qF "has U+041C CYRILLIC CAPITAL LETTER EM" \
  && ! printf '%s\n' "$out" | grep -qF "is not tracked by git"; then
  echo "ok: inherited GIT_DIR, GIT_WORK_TREE and GIT_INDEX_FILE are ignored"
  passed=$((passed + 1))
else
  echo "FAIL: inherited git variables: exit $code, want 1"
  printf '%s\n' "$out" | sed 's/^/    /'
  failures=$((failures + 1))
fi

# Retired rules keep their ID, fail nothing and are cited only in the index.
# The IDs are listed in retired-ids.txt beside the spec, so a retired rule can
# be neither deleted nor brought back.
retired_case() {
  local dir=$1 extra=$2 index=$3 list=$4
  mkdir -p "$work/$dir"
  valid_spec "$work/$dir/MANDATE.md" "$extra" "$index"
  if [ -n "$list" ]; then printf '# Retired rule IDs, never reused\n%s\n' "$list" >"$work/$dir/retired-ids.txt"; fi
}
retired_rule="#### MANDATE-ROLE-002 Retired: an old check

- Precondition: none.
- Effect: retired in spec version 1 when the check moved elsewhere; the ID is never reused.
- Error: none
"
retired_row="| \`MANDATE-ROLE-002\` | Retired: an old check |"

retired_case retired-ok "$retired_rule" "$retired_row" "MANDATE-ROLE-002"
expect "retired rule only in the index" 0 "$work/retired-ok/MANDATE.md" "--check-order" "31 rule ID(s) defined"

retired_case retired-unlisted "$retired_rule" "$retired_row" ""
expect "retired rule missing from the list" 1 "$work/retired-unlisted/MANDATE.md" "" "MANDATE-ROLE-002 is retired but not listed in retired-ids.txt"

retired_case retired-deleted "" "" "MANDATE-ROLE-002"
expect "listed retired rule deleted" 1 "$work/retired-deleted/MANDATE.md" "" "MANDATE-ROLE-002 is listed in retired-ids.txt but not defined; keep retired rules, never delete them"

retired_case retired-revived "#### MANDATE-ROLE-002 A revived check

- Precondition: something.
- Effect: refused.
- Error: \`UNAUTHORIZED\`
" "| \`MANDATE-ROLE-002\` | A revived check |" "MANDATE-ROLE-002"
expect "listed retired rule brought back" 1 "$work/retired-revived/MANDATE.md" "" "MANDATE-ROLE-002 is listed in retired-ids.txt but its title does not start with Retired:"

retired_case retired-bad-list "" "" "MANDATE-ROLE-2"
expect "malformed retired list entry" 1 "$work/retired-bad-list/MANDATE.md" "" "retired-ids.txt line 2 is not a rule ID"

f="$work/retired-error.md"
valid_spec "$f" "#### MANDATE-ROLE-002 Retired: an old check

- Precondition: none.
- Effect: retired.
- Error: \`UNAUTHORIZED\`
" "| \`MANDATE-ROLE-002\` | Retired: an old check |"
expect "retired rule with an error" 1 "$f" "" "MANDATE-ROLE-002 is retired, so its Error line must be none"

f="$work/retired-in-order.md"
valid_spec "$f" "#### MANDATE-ROLE-002 Retired: an old check

- Precondition: none.
- Effect: retired.
- Error: none

### approve

<!-- check-order: approve -->
1. The old check (MANDATE-ROLE-002).
<!-- /check-order -->" "| \`MANDATE-ROLE-002\` | Retired: an old check |"
expect "retired rule cited in a check order" 1 "$f" "--check-order" "MANDATE-ROLE-002 is retired but cited at line"

f="$work/retired-in-rule.md"
valid_spec "$f" "#### MANDATE-ROLE-002 Retired: an old check

- Precondition: none.
- Effect: retired.
- Error: none

#### MANDATE-ROLE-003 A live rule

- Precondition: something, as MANDATE-ROLE-002 used to say.
- Effect: refused.
- Error: \`UNAUTHORIZED\`
" "| \`MANDATE-ROLE-002\` | Retired: an old check |
| \`MANDATE-ROLE-003\` | A live rule |"
expect "retired rule cited in another rule" 1 "$f" "" "MANDATE-ROLE-002 is retired but cited at line"

# Check ranges ("`op` checks N to M", or "Checks N to M" after a block) must
# exist in the order they name.
f="$work/range-ok.md"
valid_spec "$f" "Checks 1 to 2 run in order.

Each item runs \`request_payout\` checks 1 to 2."
expect "check ranges inside the order pass" 0 "$f" "--check-order" "check orders: 1 block(s)"

f="$work/range-named-over.md"
valid_spec "$f" "Each item runs \`request_payout\` checks 1 to 30."
expect "named check range past the end" 1 "$f" "--check-order" "cites request_payout checks 1 to 30, but request_payout has 2 checks"

f="$work/range-after-over.md"
valid_spec "$f" "Checks 1 to 3 run for one leg."
expect "check range after a block past the end" 1 "$f" "--check-order" "cites request_payout checks 1 to 3, but request_payout has 2 checks"

f="$work/range-unknown.md"
valid_spec "$f" "Each item runs \`approve\` checks 1 to 2."
expect "check range of an unknown order" 1 "$f" "--check-order" "cites approve checks 1 to 2, but there is no approve check order"

# More range phrasings, order names of any spelling, and bare ranges kept
# within their own section.
f="$work/range-variants-ok.md"
valid_spec "$f" "Checks 1 through 2 run in order. Steps 1 to 2 also. Checks 1-2 and checks 1 and 2 too."
expect "range phrasings inside the order pass" 0 "$f" "--check-order" "check orders: 1 block(s)"

for phrase in "check 1 to 30" "checks 1 through 30" "steps 1 to 30" "checks 1-30" "checks 1 and 30"; do
  f="$work/range-variant-$(printf '%s' "$phrase" | tr ' ' '_').md"
  valid_spec "$f" "Each item runs \`request_payout\` $phrase."
  expect "range phrasing past the end: $phrase" 1 "$f" "--check-order" "but request_payout has 2 checks"
done

f="$work/range-odd-name.md"
valid_spec "$f" "Each item runs \`request_payouts_v2\` checks 1 to 2."
expect "check range naming an order that does not exist" 1 "$f" "--check-order" "cites request_payouts_v2 checks 1 to 2, but there is no request_payouts_v2 check order"

f="$work/range-other-section.md"
valid_spec "$f" "## Invariants

Checks 1 to 2 hold everywhere."
expect "bare check range outside its order's section" 1 "$f" "--check-order" "has a bare check range (Checks 1 to 2) with no check order above it in its section"

# The object store reads must return the objects asked for, in order.
mkdir -p "$work/fake-git"
real_git=$(command -v git)
cat >"$work/fake-git/git" <<FAKE
#!/bin/sh
for arg in "\$@"; do
  if [ "\$arg" = "cat-file" ]; then
    case "\$FAKE_CATFILE" in
      mismatch) "$real_git" "\$@" | sed '1s/^[0-9a-f]*/0000000000000000000000000000000000000000/'; exit 0 ;;
      truncate) "$real_git" "\$@" | head -c 60; exit 0 ;;
    esac
  fi
done
exec "$real_git" "\$@"
FAKE
chmod +x "$work/fake-git/git"
charset_case catfile-mismatch ""
make_repo "$work/catfile-mismatch"
for mode in mismatch truncate; do
  code=0
  out=$(FAKE_CATFILE=$mode PATH="$work/fake-git:$PATH" "$script" "$work/catfile-mismatch/MANDATE.md" 2>&1) || code=$?
  case $mode in
    mismatch) want="returned object 0000000000000000000000000000000000000000 where" ; name="cat-file record with a different object id exits 2" ;;
    truncate) want="cut short" ; name="cat-file record that is truncated exits 2" ;;
  esac
  if [ "$code" -eq 2 ] && printf '%s\n' "$out" | grep -qF "$want"; then
    echo "ok: $name"
    passed=$((passed + 1))
  else
    echo "FAIL: $name: exit $code, want 2"
    printf '%s\n' "$out" | sed 's/^/    /'
    failures=$((failures + 1))
  fi
done

# The machine-readable tables beside the spec must agree with it.
tables_case() {
  local dir=$1 extra=$2 errors=$3 constants=$4 events=$5
  mkdir -p "$work/$dir"
  valid_spec "$work/$dir/MANDATE.md" "$extra"
  [ -n "$errors" ] && printf '%s\n' "$errors" >"$work/$dir/errors.json"
  [ -n "$constants" ] && printf '%s\n' "$constants" >"$work/$dir/constants.json"
  [ -n "$events" ] && printf '%s\n' "$events" >"$work/$dir/EVENTS.md"
  return 0
}
good_errors='[
  { "name": "UNAUTHORIZED", "code": 10, "summary": "The caller does not hold a role this call allows" },
  { "name": "FEE_EXCEEDS_CAP", "code": 46, "summary": "The fee exceeds the fee cap" }
]'
good_constants='{ "constants": {}, "role_codes": { "AGENT": 1 }, "recovery_action_kinds": { "REVOKE": 1 } }'
good_events='# Events

| Event | Fields | Topic key | Data |
| --- | --- | --- | --- |'

tables_case tables-ok "" "$good_errors" "$good_constants" "$good_events"
expect "tables that agree with the spec pass" 0 "$work/tables-ok/MANDATE.md" "--check-order" "error table, constants and events agree"

tables_case tables-dup-code "" '[
  { "name": "UNAUTHORIZED", "code": 10, "summary": "x" },
  { "name": "FEE_EXCEEDS_CAP", "code": 10, "summary": "y" }
]' "$good_constants" "$good_events"
expect "duplicate error code" 1 "$work/tables-dup-code/MANDATE.md" "" "errors.json lists code 10 twice"

tables_case tables-dup-constant "## Constants

| Name | Value | Meaning |
| --- | --- | --- |
| \`MAX_LEGS\` | 4 | Legs |" "$good_errors" '{ "constants": { "MAX_LEGS": 4, "MAX_LEGS": 5 }, "role_codes": {}, "recovery_action_kinds": {} }' "$good_events"
expect "duplicate constant" 1 "$work/tables-dup-constant/MANDATE.md" "" "constants.json has the key MAX_LEGS twice"

tables_case tables-missing-error "" '[
  { "name": "UNAUTHORIZED", "code": 10, "summary": "x" }
]' "$good_constants" "$good_events"
expect "rule cites unknown error" 1 "$work/tables-missing-error/MANDATE.md" "" "error FEE_EXCEEDS_CAP (46) is in the MANDATE.md error table but not in errors.json"

tables_case tables-unknown-event "Effects emit \`ghost_event\` once." "$good_errors" "$good_constants" "$good_events"
expect "rule cites unknown event" 1 "$work/tables-unknown-event/MANDATE.md" "" "event ghost_event is cited at line"

tables_case tables-event-fields "### Event list

| Event | Fields |
| --- | --- |
| \`paused\` | by |" "$good_errors" "$good_constants" "# Events

| Event | Fields | Topic key | Data |
| --- | --- | --- | --- |
| \`paused\` | by, at | tenant_ref | by, at |"
expect "event fields differ" 1 "$work/tables-event-fields/MANDATE.md" "" "event paused has fields by in MANDATE.md but by, at in EVENTS.md"

event_list_row="### Event list

| Event | Fields |
| --- | --- |
| \`payout_approved\` | payout_id, by |"
tables_case tables-data-ok "$event_list_row" "$good_errors" "$good_constants" "# Events

| Event | Fields | Topic key | Data |
| --- | --- | --- | --- |
| \`payout_approved\` | payout_id, by | payout_id | by |"
expect "event data without its topic key passes" 0 "$work/tables-data-ok/MANDATE.md" "" "error table, constants and events agree"

tables_case tables-topic-in-data "$event_list_row" "$good_errors" "$good_constants" "# Events

| Event | Fields | Topic key | Data |
| --- | --- | --- | --- |
| \`payout_approved\` | payout_id, by | payout_id | payout_id, by |"
expect "topic key repeated in the data" 1 "$work/tables-topic-in-data/MANDATE.md" "" "event payout_approved repeats its topic key payout_id in the data"

tables_case tables-data-mismatch "$event_list_row" "$good_errors" "$good_constants" "# Events

| Event | Fields | Topic key | Data |
| --- | --- | --- | --- |
| \`payout_approved\` | payout_id, by | payout_id | approver |"
expect "event data not the fields without the topic key" 1 "$work/tables-data-mismatch/MANDATE.md" "" "event payout_approved has data approver, but its fields without the topic key are by"

tables_case tables-constant-value "## Constants

| Name | Value | Meaning |
| --- | --- | --- |
| \`I128_MAX\` | 170141183460469231731687303715884105727 | Largest amount |" "$good_errors" '{ "constants": { "I128_MAX": "170141183460469231731687303715884105726" }, "role_codes": {}, "recovery_action_kinds": {} }' "$good_events"
expect "constant value differs" 1 "$work/tables-constant-value/MANDATE.md" "" "constant I128_MAX is 170141183460469231731687303715884105727 in MANDATE.md but 170141183460469231731687303715884105726 in constants.json"

tables_case tables-bad-json "" '[ { "name": "UNAUTHORIZED", "code": 10, ' "$good_constants" "$good_events"
expect "malformed errors.json" 1 "$work/tables-bad-json/MANDATE.md" "" "errors.json is not valid JSON"

tables_case tables-required "" "" "" ""
expect "tables required" 1 "$work/tables-required/MANDATE.md" "--require-tables" "errors.json is missing"

no_repo=1 expect "unreadable file" 2 "$work/does-not-exist.md" ""
expect "unknown option" 2 "$work/valid.md" "--frobnicate"

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
