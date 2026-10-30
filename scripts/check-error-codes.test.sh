#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Self test for check-error-codes.py. Each case runs the check on fixtures
# under scripts/fixtures/error-codes/ and checks the exit code, and that the
# output names the problem, so a case cannot pass by failing for some other
# reason. The last cases plant a deliberate mismatch in copies of the real
# enum and the real table, so the check is proven on the files it guards and
# not only on fixtures.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
script="$here/check-error-codes.py"
fx="$here/fixtures/error-codes"
real_rust="$root/soroban/crates/mandate-errors/src/lib.rs"
real_spec="$root/spec/errors.json"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

passed=0
failures=0

pass() {
  echo "ok: $1"
  passed=$((passed + 1))
}

fail() {
  echo "FAIL: $1"
  printf '%s\n' "$2" | sed 's/^/    /'
  failures=$((failures + 1))
}

# check <case name> <exit code> [text the output must contain]... -- [arguments]...
check() {
  local name=$1 want=$2 needles=() needle out code=0 verdict
  shift 2
  while [ $# -gt 0 ] && [ "$1" != "--" ]; do
    needles+=("$1")
    shift
  done
  shift
  out=$("$script" "$@" 2>&1) || code=$?
  case $want in
    0) verdict="passes" ;;
    1) verdict="refused as intended" ;;
    *) verdict="exits $want as intended" ;;
  esac
  if [ "$code" -ne "$want" ]; then
    fail "$name: exit $code, want $want" "$out"
    return
  fi
  # A here-string, not a pipe: under pipefail, grep -q exiting at its first
  # match can kill the writer with SIGPIPE and turn a match into a failure.
  for needle in ${needles[@]+"${needles[@]}"}; do
    if ! grep -qF -- "$needle" <<<"$out"; then
      fail "$name: the output does not mention '$needle'" "$out"
      return
    fi
  done
  pass "$name ($verdict)"
}

# The real tree. The code count is left out because the table only grows.
check "the real tree passes" 0 \
  "code(s) in spec/errors.json match enum MandateError in soroban/crates/mandate-errors/src/lib.rs" --
check "codes 26, 27 and 65 are in parity" 0 \
  "26 PRINCIPAL_CHANGE_PENDING PrincipalChangePending" \
  "27 SWEEP_GRACE_PERIOD SweepGracePeriod" \
  "65 AGENT_REMOVED AgentRemoved" -- --list

# The four kinds of drift, each on its own and then all in one run.
spec=(--spec "$fx/errors.json")
check "matching fixture" 0 "the 4 code(s) in" -- "${spec[@]}" --rust "$fx/match.rs"
check "missing in rust" 1 "missing in rust: AGENT_REMOVED (65)" "add AgentRemoved = 65" -- \
  "${spec[@]}" --rust "$fx/missing-in-rust.rs"
check "extra in rust" 1 "extra in rust: AGENT_SUSPENDED (66), AgentSuspended at" -- \
  "${spec[@]}" --rust "$fx/extra-in-rust.rs"
check "renumbered code" 1 "renumbered code: AGENT_REMOVED is 65 in" "but 66 in" -- \
  "${spec[@]}" --rust "$fx/renumbered-code.rs"
check "renamed entry" 1 "renamed entry: code 65 is AGENT_REMOVED in" "but AGENT_DROPPED (AgentDropped) in" -- \
  "${spec[@]}" --rust "$fx/renamed-entry.rs"
check "every kind named in one run" 1 "4 difference(s)" \
  "missing in rust: UNAUTHORIZED (10)" \
  "renumbered code: PRINCIPAL_CHANGE_PENDING is 26 in" \
  "renamed entry: code 60 is PAYOUT_ID_EXISTS in" \
  "extra in rust: AGENT_SUSPENDED (66)" -- "${spec[@]}" --rust "$fx/every-kind.rs"

# How the enum is read.
check "variants in comments do not count" 1 \
  "missing in rust: PAYOUT_ID_EXISTS (60)" "missing in rust: AGENT_REMOVED (65)" -- \
  "${spec[@]}" --rust "$fx/commented-out.rs"
check "an acronym does not match the table name" 1 \
  "renamed entry: code 60 is PAYOUT_ID_EXISTS in" "but PAYOUT_IDEXISTS (PayoutIDExists)" -- \
  "${spec[@]}" --rust "$fx/acronym.rs"
check "a hex code" 1 "cannot read 'AgentRemoved = 0x41'" -- "${spec[@]}" --rust "$fx/hex-code.rs"
check "an attribute on a variant" 1 "cannot read '#[cfg(test)] AgentRemoved = 65'" -- \
  "${spec[@]}" --rust "$fx/attribute-in-body.rs"
check "a variant without a code" 1 "cannot read 'AgentRemoved'" -- "${spec[@]}" --rust "$fx/no-code.rs"
check "a code used twice in rust" 1 "lists code 65 twice, for AgentRemoved and AgentGone" -- \
  "${spec[@]}" --rust "$fx/duplicate-code.rs"
check "an enum that is not repr(u32)" 1 "is not #[repr(u32)]" -- "${spec[@]}" --rust "$fx/no-repr.rs"

printf '// SPDX-License-Identifier: Apache-2.0\npub enum MandateError {\n    AgentRemoved = 65,\n}\n' >"$work/no-enum.rs"
check "no contracterror enum" 2 "no #[contracterror] enum in" -- "${spec[@]}" --rust "$work/no-enum.rs"
cat "$fx/match.rs" "$fx/match.rs" >"$work/two-enums.rs"
check "two contracterror enums" 2 "2 #[contracterror] enums in" -- "${spec[@]}" --rust "$work/two-enums.rs"
printf '#[contracterror]\n#[repr(u32)]\npub enum MandateError {\n    /* AgentRemoved = 65,\n}\n' >"$work/open-comment.rs"
check "a block comment that never ends" 2 "a block comment never ends" -- \
  "${spec[@]}" --rust "$work/open-comment.rs"

# How the table is read.
check "a table that lists a code twice" 1 "lists code 65 twice, for AGENT_REMOVED and AGENT_GONE" -- \
  --spec "$fx/duplicate-code.json" --rust "$fx/match.rs"
check "a table that is not a list" 2 "must be a list of { name, code, summary }" -- \
  --spec "$fx/not-a-list.json" --rust "$fx/match.rs"
printf '[{"name": "AGENT_REMOVED", "code": 65, "code": 66, "summary": "x"}]\n' >"$work/repeated-key.json"
check "a table entry with a repeated key" 2 "the key code appears twice in one object" -- \
  --spec "$work/repeated-key.json" --rust "$fx/match.rs"
check "a missing file" 2 "cannot read $work/none.json" -- --spec "$work/none.json"
check "an unknown option" 2 "unrecognized arguments: --bogus" -- --bogus

# Deliberate mismatches in copies of the real files. Each copy is compared
# with its original first, so a reworded line cannot leave the copy unchanged
# and let the case pass for the wrong reason.
planted() {
  if cmp -s "$1" "$2"; then
    fail "$3: the planted change did not apply" "$2 is identical to $1"
    return 1
  fi
}
sed 's/^\( *AgentRemoved = \)65,$/\166,/' "$real_rust" >"$work/renumbered.rs"
if planted "$real_rust" "$work/renumbered.rs" "real enum with a code changed"; then
  check "real enum with a code changed" 1 \
    "renumbered code: AGENT_REMOVED is 65 in spec/errors.json but 66 in" -- --rust "$work/renumbered.rs"
fi
grep -v '^ *SweepGracePeriod = 27,$' "$real_rust" >"$work/missing.rs" || true
if planted "$real_rust" "$work/missing.rs" "real enum with a variant removed"; then
  check "real enum with a variant removed" 1 "missing in rust: SWEEP_GRACE_PERIOD (27)" -- \
    --rust "$work/missing.rs"
fi
python3 - "$real_spec" "$work/appended.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as fh:
    table = json.load(fh)
table.append({"name": "PLANTED_ERROR", "code": 99, "summary": "Added to the table only"})
with open(sys.argv[2], "w", encoding="utf-8") as fh:
    json.dump(table, fh, indent=2)
PY
check "real table with an entry the enum lacks" 1 "missing in rust: PLANTED_ERROR (99)" "add PlantedError = 99" -- \
  --spec "$work/appended.json"

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
