#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Checks that the pull request template still asks for everything a pull
# request here must show: the linked issue, what and why, tests, docs, the
# checklist, the reviews the change type needs and the merge rules. Also
# checks the template is plain ASCII.
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

for heading in 'Linked issue' 'What and why' 'Tests' 'Docs' 'Checklist' 'Required reviews' 'Before merge'; do
  grep -q -x -F -e "## $heading" "$template" || need "missing section: ## $heading"
done

# Each of these must be an unticked checkbox item. Matched against a
# here-string, not a pipe: under pipefail an early exit of grep -q could
# fail the writer.
checkboxes=$(grep -E '^- \[ \] ' "$template" || true)
for item in 'Tests' 'Docs' 'No em or en dashes' 'Plain ASCII' 'No secrets' 'Zero warnings' \
  'integer minor units' 'failing test'; do
  grep -q -F -e "$item" <<<"$checkboxes" || need "missing checklist item: $item"
done

grep -q -F -e 'Closes #' "$template" || need "missing text: Closes #"

# The review rules and merge rules must be present word for word, so a
# category cannot be narrowed or dropped without failing here.
while IFS= read -r line; do
  grep -q -x -F -e "$line" "$template" || need "missing or changed line: $line"
done <<'LINES'
- Every change: code review, QA verification and a hygiene review.
- Auth, identity, payouts, idempotency, router, secrets, dependencies, CI and infrastructure: add a security review.
- Contracts: add an exploit review and an audit review.
- Dashboard, website, embed and `packages/ui`: add a design review.
- Any new commit after an approval needs review again.
- Merges are squash only.
LINES

if [ "$missing" -ne 0 ]; then
  exit 1
fi
echo "check-pr-template: $(basename "$template") has every required part"
