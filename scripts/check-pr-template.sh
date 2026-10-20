#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Shared file: the same copy lives in ujira, ujira-contracts, ujira-docs and
# ujira-mcp (qomiralabs/ujira#304). Change all four together.
#
# Checks that the pull request template still asks for everything a pull
# request here must show: the linked issue, what and why, tests, docs, the
# checklist, the reviews the change type needs and the merge rules. Also
# checks the template is plain ASCII.
#
# Only text a reader is asked to fill in counts: HTML comments and fenced
# code blocks are stripped first, so a required part that is commented out or
# shown as an example does not satisfy the check. Each required line must also
# sit under its own heading, so a review rule in the checklist or a stray
# "Closes #" in the reviews text does not count either.
#
# Usage: scripts/check-pr-template.sh [template, default
# .github/pull_request_template.md]
# Exit codes: 0 complete, 1 something is missing.
set -euo pipefail

here=$(cd "$(dirname "$0")/.." && pwd)
template=${1:-$here/.github/pull_request_template.md}

if [ ! -f "$template" ]; then
  echo "check-pr-template: $template is missing" >&2
  exit 1
fi

missing=0
need() { echo "check-pr-template: $1" >&2; missing=1; }

if LC_ALL=C grep -n '[^ -~]' "$template" >&2; then
  need "non ASCII or control characters on the lines above"
fi

# Drop HTML comments and fenced code blocks: text a reader never fills in
# cannot satisfy a requirement. Lines are kept, blanked rather than deleted,
# so a heading inside a comment cannot pull the lines under it into the
# previous section.
visible=$(python3 -I -c '
import re, sys
text = open(sys.argv[1], encoding="utf-8").read()
text = re.sub(r"<!--.*?-->", lambda m: "\n" * m.group(0).count("\n"), text, flags=re.S)
out, fenced = [], False
for line in text.split("\n"):
    if re.match(r"\s*(?:```|~~~)", line):
        fenced = not fenced
        out.append("")
        continue
    out.append("" if fenced else line)
sys.stdout.write("\n".join(out))
' "$template")

# section <heading>: the lines under "## <heading>", up to the next heading.
section() {
  awk -v want="## $1" '
    /^## / { inside = ($0 == want); next }
    inside { print }
  ' <<<"$visible"
}

for heading in 'Linked issue' 'What and why' 'Tests' 'Docs' 'Checklist' 'Required reviews' 'Before merge'; do
  grep -q -x -F -e "## $heading" <<<"$visible" || need "missing section: ## $heading"
done

# Each of these must be an unticked checkbox under the checklist heading, with
# its whole text, so a shortened or reworded item fails. Matched against a
# here-string, not a pipe: under pipefail an early exit of grep -q could fail
# the writer.
checkboxes=$(grep -E '^- \[ \] ' <<<"$(section Checklist)" || true)
while IFS= read -r item; do
  [ -n "$item" ] || continue
  grep -q -x -F -e "- [ ] $item" <<<"$checkboxes" || need "missing or changed checklist item: $item"
done <<'ITEMS'
Tests: written before or with the code, run, and passing, with the output above
Docs: updated, or no docs change with the reason above
No em or en dashes anywhere in the change, the commits or this description
Plain ASCII only in this description
No secrets, keys or tokens in code, history, logs or this description
Zero warnings from the compiler, linters and tests
Money in integer minor units with an explicit currency, never a float
A bug fix starts with a failing test that reproduces it
Every review this change type needs (below) is requested
ITEMS

grep -q -F -e 'Closes #' <<<"$(section 'Linked issue')" \
  || need "missing text under ## Linked issue: Closes #"

# The review rules and the merge rules must be present word for word, and
# under their own heading, so a category cannot be narrowed, dropped or moved
# somewhere it asks nothing.
reviews=$(section 'Required reviews')
while IFS= read -r line; do
  grep -q -x -F -e "$line" <<<"$reviews" || need "missing or changed line under ## Required reviews: $line"
done <<'LINES'
- Every change: code review, QA verification and a hygiene review.
- Auth, identity, payouts, idempotency, router, secrets, dependencies, CI and infrastructure: add a security review.
- Contracts: add an exploit review and an audit review.
- Dashboard, website, embed and `packages/ui`: add a design review.
LINES

before_merge=$(section 'Before merge')
while IFS= read -r line; do
  grep -q -x -F -e "$line" <<<"$before_merge" || need "missing or changed line under ## Before merge: $line"
done <<'LINES'
- Any new commit after an approval needs review again.
- Merges are squash only.
LINES

if [ "$missing" -ne 0 ]; then
  exit 1
fi
echo "check-pr-template: $(basename "$template") has every required part"
