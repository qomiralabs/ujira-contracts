#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Shared file: the same copy lives in ujira, ujira-contracts, ujira-docs and
# ujira-mcp (qomiralabs/ujira#304). Change all four together.
#
# Self test for check-pr-template.sh. It checks the committed template, then
# copies with one required part removed, moved or hidden, each of which must
# fail.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
script="$here/scripts/check-pr-template.sh"
template="$here/.github/pull_request_template.md"
work=$(cd "$(mktemp -d)" && pwd -P)
trap 'rm -rf "$work"' EXIT

passed=0
failures=0

# expect <case name> <wanted exit code> <template path>
expect() {
  local name=$1 want=$2 got=0 out
  out=$("$script" "$3" 2>&1) || got=$?
  if [ "$got" -eq "$want" ]; then
    passed=$((passed + 1))
    echo "PASS  $name"
  else
    failures=$((failures + 1))
    echo "FAIL  $name (wanted exit $want, got $got)"
    printf '%s\n' "$out" | sed 's/^/      /'
  fi
}

# without <name> <fixed string>: a copy of the template without the lines
# that contain the string.
without() {
  grep -v -F -e "$2" "$template" >"$work/$1.md" || true
  printf '%s' "$work/$1.md"
}

expect "the committed template passes" 0 "$template"
expect "a missing template fails" 1 "$work/none.md"

expect "no linked issue section fails" 1 "$(without issue '## Linked issue')"
expect "no what and why section fails" 1 "$(without what '## What and why')"
expect "no tests section fails" 1 "$(without tests '## Tests')"
expect "no docs section fails" 1 "$(without docs '## Docs')"
expect "no checklist section fails" 1 "$(without checklist '## Checklist')"
expect "no required reviews section fails" 1 "$(without reviews '## Required reviews')"
expect "no before merge section fails" 1 "$(without merge '## Before merge')"
expect "no Closes line fails" 1 "$(without closes 'Closes #')"
expect "no dash item fails" 1 "$(without dash 'No em or en dashes')"
expect "no ASCII item fails" 1 "$(without ascii 'Plain ASCII')"
expect "no secrets item fails" 1 "$(without secret 'No secrets')"
expect "no warnings item fails" 1 "$(without warn 'Zero warnings')"
expect "no money item fails" 1 "$(without money 'integer minor units')"
expect "no failing test item fails" 1 "$(without bugfix 'failing test')"
expect "no security review rule fails" 1 "$(without security 'security review')"
expect "no exploit and audit rule fails" 1 "$(without exploit 'exploit review')"
expect "no design review rule fails" 1 "$(without design 'design review')"
expect "no re-review reminder fails" 1 "$(without rereview 'needs review again')"
expect "no squash only reminder fails" 1 "$(without squash 'squash')"

# Each change type that adds a review must keep its full list. Narrowing a
# line, or dropping a category from it, must fail.
sed 's/^- Auth, identity, payouts, idempotency, router, secrets, dependencies, CI and infrastructure: add a security review\.$/- Infrastructure: add a security review./' \
  "$template" >"$work/narrow-security.md"
expect "a narrowed security review line fails" 1 "$work/narrow-security.md"
sed 's/^- Contracts: add an exploit review and an audit review\.$/- Contracts: add an exploit review./' \
  "$template" >"$work/narrow-contracts.md"
expect "a contracts line without the audit review fails" 1 "$work/narrow-contracts.md"
# The backticks are Markdown code spans, not shell expansions.
# shellcheck disable=SC2016
sed 's/^- Dashboard, website, embed and `packages\/ui`: add a design review\.$/- Dashboard: add a design review./' \
  "$template" >"$work/narrow-design.md"
expect "a narrowed design review line fails" 1 "$work/narrow-design.md"
sed 's/^- Every change: code review, QA verification and a hygiene review\.$/- Every change: code review./' \
  "$template" >"$work/narrow-every.md"
expect "a narrowed every change line fails" 1 "$work/narrow-every.md"

# A checklist item must be a checkbox, not plain text.
sed 's/^- \[ \] No secrets/- No secrets/' "$template" >"$work/plain.md"
expect "a checklist item without a checkbox fails" 1 "$work/plain.md"

# A required part that is commented out or inside a code fence is not asking
# for anything, so it must not count.
hide_comment() {
  python3 -I -c '
import sys
text = open(sys.argv[1], encoding="utf-8").read()
line = sys.argv[2]
assert line in text, line
sys.stdout.write(text.replace(line, "<!-- " + line + " -->"))
' "$1" "$2"
}

hide_comment "$template" \
  "- Auth, identity, payouts, idempotency, router, secrets, dependencies, CI and infrastructure: add a security review." \
  >"$work/commented.md"
expect "required section inside an HTML comment" 1 "$work/commented.md"

# The single quotes keep the Python source literal; nothing expands.
# shellcheck disable=SC2016
python3 -I -c '
import sys
text = open(sys.argv[1], encoding="utf-8").read()
heading = "## Required reviews"
assert heading in text
sys.stdout.write(text.replace(heading, "```\n" + heading + "\n```"))
' "$template" >"$work/fenced.md"
expect "required section inside a code fence" 1 "$work/fenced.md"

# A review rule only means something under the reviews heading.
python3 -I -c '
import sys
text = open(sys.argv[1], encoding="utf-8").read()
line = "- Auth, identity, payouts, idempotency, router, secrets, dependencies, CI and infrastructure: add a security review."
assert line in text
text = text.replace(line + "\n", "")
text = text.replace("## Checklist\n", "## Checklist\n\n" + line + "\n")
sys.stdout.write(text)
' "$template" >"$work/misplaced-review.md"
expect "security review line under the checklist heading" 1 "$work/misplaced-review.md"

# A checklist item must keep its whole text, not just its first words.
sed 's/^- \[ \] No em or en dashes.*$/- [ ] No em/' "$template" >"$work/short-item.md"
expect "checklist item shortened to no em" 1 "$work/short-item.md"

# The linked issue placeholder must be under its own heading, not merely
# somewhere in the file.
python3 -I -c '
import sys
text = open(sys.argv[1], encoding="utf-8").read()
assert "Closes #" in text
text = text.replace("Closes #\n", "The issue this closes.\n")
text = text.replace("## Required reviews\n", "## Required reviews\n\nUse Closes #N in the linked issue section.\n")
sys.stdout.write(text)
' "$template" >"$work/misplaced-closes.md"
expect "Closes # only inside the reviews section" 1 "$work/misplaced-closes.md"

# The merge rules only mean something under the before merge heading.
python3 -I -c '
import sys
text = open(sys.argv[1], encoding="utf-8").read()
line = "- Merges are squash only."
assert line in text
text = text.replace(line + "\n", "")
text = text.replace("## Tests\n", "## Tests\n\n" + line + "\n")
sys.stdout.write(text)
' "$template" >"$work/misplaced-merge.md"
expect "squash only reminder outside the before merge section" 1 "$work/misplaced-merge.md"

# A heading must be a real heading, not a line that only mentions one.
python3 -I -c '
import sys
text = open(sys.argv[1], encoding="utf-8").read()
assert "## Docs\n" in text
sys.stdout.write(text.replace("## Docs\n", "Write about ## Docs here.\n"))
' "$template" >"$work/not-a-heading.md"
expect "a heading mentioned in prose does not count" 1 "$work/not-a-heading.md"

# An unterminated comment hides everything after it when GitHub renders the
# template, so the check must treat it as running to the end of the file.
# shellcheck disable=SC2016
python3 -I -c '
import sys
text = open(sys.argv[1], encoding="utf-8").read()
heading = "## Required reviews"
assert heading in text
sys.stdout.write(text.replace(heading, "<!-- everything from here is hidden\n" + heading))
' "$template" >"$work/unterminated.md"
expect "an unterminated comment hides the rest of the template" 1 "$work/unterminated.md"

# The comment markers on their own lines leave the required line intact, so
# this case passes only while the stripping is in place. Remove the stripping
# and it fails, which is what makes it a guard for it.
# shellcheck disable=SC2016
python3 -I -c '
import sys
text = open(sys.argv[1], encoding="utf-8").read()
line = "- Auth, identity, payouts, idempotency, router, secrets, dependencies, CI and infrastructure: add a security review."
assert line in text
sys.stdout.write(text.replace(line, "<!--\n" + line + "\n-->"))
' "$template" >"$work/commented-block.md"
expect "a review line inside a comment block does not count" 1 "$work/commented-block.md"

# The same for a fenced block around a required line, not just a heading.
# shellcheck disable=SC2016
python3 -I -c '
import sys
text = open(sys.argv[1], encoding="utf-8").read()
line = "- Merges are squash only."
assert line in text
sys.stdout.write(text.replace(line, "```\n" + line + "\n```"))
' "$template" >"$work/fenced-line.md"
expect "a merge rule inside a code fence does not count" 1 "$work/fenced-line.md"

# A checklist item must be the whole line, so trailing text that waters it
# down fails. A substring match would let this through.
sed 's/^- \[ \] No em or en dashes anywhere in the change, the commits or this description$/- [ ] No em or en dashes anywhere in the change, the commits or this description, except where a quotation needs one/' \
  "$template" >"$work/padded-item.md"
expect "a checklist item with extra text appended fails" 1 "$work/padded-item.md"

# The template must be plain ASCII.
{ cat "$template"; printf 'Caf\303\251\n'; } >"$work/nonascii.md"
expect "a non ASCII character fails" 1 "$work/nonascii.md"

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
