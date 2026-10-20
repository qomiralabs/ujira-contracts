#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Self test for check-pr-template.sh. It checks the committed template, then
# copies with one required part removed, each of which must fail.
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

# The template must be plain ASCII.
{ cat "$template"; printf 'Caf\303\251\n'; } >"$work/nonascii.md"
expect "a non ASCII character fails" 1 "$work/nonascii.md"

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
