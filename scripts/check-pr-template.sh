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

# Each of these must be an unticked checkbox item.
for item in 'Tests' 'Docs' 'No em or en dashes' 'Plain ASCII' 'No secrets' 'Zero warnings' \
  'integer minor units' 'failing test'; do
  grep -E '^- \[ \] ' "$template" | grep -q -F -e "$item" || need "missing checklist item: $item"
done

for text in 'Closes #' 'code review' 'QA' 'hygiene review' 'security review' 'exploit review' \
  'audit review' 'design review' 'needs review again' 'squash'; do
  grep -q -F -e "$text" "$template" || need "missing text: $text"
done

if [ "$missing" -ne 0 ]; then
  exit 1
fi
echo "check-pr-template: $(basename "$template") has every required part"
