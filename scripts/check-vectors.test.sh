#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Self test for check-vectors.sh. Each case runs the check on fixtures under
# scripts/fixtures/vectors/ and checks the exit code, and that the output
# names the problem, so a case cannot pass by failing for some other reason.
# The fixtures run against the repository's own spec, except the coverage
# cases, which use the small spec in scripts/fixtures/vectors/spec/.
#
# Needs check-jsonschema at the pinned version, found the same way
# check-vectors.sh finds it: CHECK_JSONSCHEMA, or check-jsonschema on PATH.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
script="$here/check-vectors.sh"
fx="$here/fixtures/vectors"
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
  for needle in ${needles[@]+"${needles[@]}"}; do
    if ! printf '%s\n' "$out" | grep -qF -- "$needle"; then
      fail "$name: the output does not mention '$needle'" "$out"
      return
    fi
  done
  pass "$name ($verdict)"
}

# The real tree, and the example the README shows. The vector and file counts
# are left out because every vector set added under spec/vectors/ changes them.
check "the real tree passes" 0 "file(s) pass the checks against the spec" "every check passed" --
check "example vector validates" 0 "every check passed" -- "$root/spec/vectors/example.json"
if python3 -I - "$root/spec/vectors/README.md" "$root/spec/vectors/example.json" <<'PY'; then
import json
import re
import sys

with open(sys.argv[1], encoding="ascii") as fh:
    blocks = re.findall(r"^```json\n(.*?)^```$", fh.read(), re.S | re.M)
with open(sys.argv[2], encoding="ascii") as fh:
    want = json.load(fh)
sys.exit(0 if len(blocks) == 1 and json.loads(blocks[0]) == want else 1)
PY
  pass "the README example is example.json (passes)"
else
  fail "the README example is example.json" "spec/vectors/README.md must hold exactly one json block, equal to spec/vectors/example.json"
fi
check "valid fixture passes" 0 "every check passed" -- "$fx/base.json"

# Amounts are decimal strings of at most 39 digits with no leading zero. A
# negative amount is Stellar only and an amount above I128_MAX is Celo only.
check "numeric amount" 1 "args.amount: 100 is not of type 'string'" -- "$fx/numeric-amount.json"
check "amount too long" 1 "args.amount: '1000000000000000000000000000000000000000' does not match" -- "$fx/amount-too-long.json"
check "negative amount on both chains" 1 "is the negative amount -1, which only Stellar can express" -- "$fx/negative-amount-both-chains.json"
check "negative amount on stellar only" 0 "every check passed" -- "$fx/negative-amount-stellar-only.json"
check "leading zero" 1 "args.amount: '0100' does not match" -- "$fx/leading-zero.json"
check "amount above I128_MAX on both chains" 1 "above I128_MAX, which only Celo can express" -- "$fx/amount-above-i128-both-chains.json"
check "amount above I128_MAX on celo only" 0 "every check passed" -- "$fx/amount-above-i128-celo-only.json"

# Tags are u64 decimal strings or null; the non-canonical form is Celo only,
# and only an account takes a tag.
check "numeric tag" 1 "destination.tag: 7 is not of type 'null', 'string', 'object'" -- "$fx/numeric-tag.json"
check "tag above u64" 1 "destination.tag: '18446744073709551616' does not match" -- "$fx/tag-above-u64.json"
check "largest u64 tag" 0 "every check passed" -- "$fx/largest-u64-tag.json"
check "non-canonical tag on both chains" 1 "is the non-canonical tag form, which only Celo can express" -- "$fx/noncanonical-tag-both-chains.json"
check "null tag" 0 "every check passed" -- "$fx/null-tag.json"
check "non-canonical tag on celo only" 0 "every check passed" -- "$fx/noncanonical-tag-celo-only.json"
check "tagged contract destination" 1 "gives vault a tag, but only an account actor can take one" -- "$fx/tagged-contract-destination.json"

# Restricting a vector to some chains needs the one skip reason, and the
# reverse.
check "applies_to without skip reason" 1 "'skip_reason' is a dependency of 'applies_to'" -- "$fx/applies-to-without-skip-reason.json"
check "skip reason without applies_to" 1 "'applies_to' is a dependency of 'skip_reason'" -- "$fx/skip-reason-without-applies-to.json"
check "applies_to listing every chain" 1 "applies_to: ['stellar', 'celo'] should not be valid under" -- "$fx/applies-to-every-chain.json"
check "zero address on both chains" 1 "is the zero address, which only Celo can express" -- "$fx/zero-address-both-chains.json"
check "zero address on celo only" 0 "every check passed" -- "$fx/zero-address-celo-only.json"

# Names come from the spec.
check "unknown rule" 1 "cites MANDATE-PAUSE-099, which is not a rule in MANDATE.md" -- "$fx/unknown-rule.json"
check "vector citing MANDATE-RECIPIENT-011" 1 "cites MANDATE-RECIPIENT-011, which is retired" -- "$fx/retired-rule.json"
check "unknown error" 1 "expects error NOT_AN_ERROR, which is not in errors.json" -- "$fx/unknown-error.json"
check "unknown event" 1 "expects event not_an_event, which is not in EVENTS.md" -- "$fx/unknown-event.json"
check "event without its fields" 1 "event 1 (paused) has the fields none, but EVENTS.md gives it by" -- "$fx/event-missing-field.json"
check "unknown tamper" 1 "tamper: 'swapped_bytes' is not one of" -- "$fx/unknown-tamper.json"

# Attestations are ed25519. The secp256k1 tamper kinds are retired, by name
# and in the parameterised bad_v form.
check "high_s tamper is refused" 1 "tamper: 'high_s' is not one of" "tamper high_s is a retired secp256k1 kind" -- "$fx/high-s-tamper.json"
for kind in bad_v r_zero s_zero r_order r_not_on_curve recovers_to_infinity; do
  sed "s/\"high_s\"/\"$kind\"/" "$fx/high-s-tamper.json" >"$work/retired-$kind.json"
  check "$kind tamper is refused" 1 "tamper $kind is a retired secp256k1 kind" -- "$work/retired-$kind.json"
done
check "bad_v with a v value is refused" 1 "tamper bad_v is a retired secp256k1 kind" -- "$fx/bad-v-with-value.json"
check "short_signature tamper validates" 0 "every check passed" -- "$fx/short-signature-tamper.json"
check "attestation no attest step made" 1 "uses the attestation att9, which no earlier attest step made" -- "$fx/unknown-attestation.json"

# A Stellar host_abort is allowed for every tamper that leaves a 64-byte
# signature failing verification, and for nothing else.
check "host_abort on wrong_signer validates" 0 "every check passed" -- "$fx/host-abort-wrong-signer.json"
for kind in wrong_chain wrong_mandate wrong_destination wrong_tag wrong_policy wrong_ref wrong_valid_until flipped_bit non_canonical_s; do
  sed "s/\"wrong_signer\"/\"$kind\"/" "$fx/host-abort-wrong-signer.json" >"$work/abort-$kind.json"
  check "host_abort on $kind validates" 0 "every check passed" -- "$work/abort-$kind.json"
done
check "host_abort on short_signature" 1 "no attestation it carries has an invalid-signature tamper (short_signature)" -- "$fx/host-abort-short-signature.json"
for kind in empty truncated long_signature extended; do
  sed "s/\"short_signature\"/\"$kind\"/" "$fx/host-abort-short-signature.json" >"$work/abort-$kind.json"
  check "host_abort on $kind" 1 "no attestation it carries has an invalid-signature tamper ($kind)" -- "$work/abort-$kind.json"
done
check "host_abort on an untampered attestation" 1 "no attestation it carries has an invalid-signature tamper (none is tampered)" -- "$fx/host-abort-untampered.json"
check "host_abort on a call that verifies nothing" 1 "expects a Stellar host_abort from pause" -- "$fx/host-abort-on-pause.json"
check "request_payouts host_abort with payout_request_rejected events" 1 "so it cannot also expect payout_request_rejected events" -- "$fx/request-payouts-host-abort-rejected-events.json"
check "request_payouts host_abort failing as a whole validates" 0 "every check passed" -- "$fx/request-payouts-host-abort.json"

# Host errors that are factory bugs cannot be expected, and host
# authorization failures only where the spec puts them.
check "create expecting a host context error" 1 "'Crypto.InvalidInput' was expected" "expects a Stellar host_abort from create" -- "$fx/create-host-context-error.json"
check "host_auth_failure on a vector that does not cite caller authorization" 1 "only a vector that cites MANDATE-ROLE-019 may" -- "$fx/host-auth-failure-without-caller-rule.json"
check "host_auth_failure on a vector citing caller authorization" 0 "every check passed" -- "$fx/host-auth-failure-with-caller-rule.json"
check "host_auth_failure on a call with no caller argument" 0 "every check passed" -- "$fx/host-auth-failure-no-caller-argument.json"
check "host_auth_failure on a call its caller signs" 1 "the call is signed by its own caller" -- "$fx/host-auth-failure-self-signed.json"
check "host_auth_failure on a view" 1 "from the view state, which needs no authorization" -- "$fx/host-auth-failure-view.json"

# Test tokens that only one chain can express.
check "reentrant token on both chains" 1 "token asset is a reentrant test token, which only Celo can express" -- "$fx/reentrant-token-both-chains.json"
check "reentrant token on celo only" 0 "every check passed" -- "$fx/reentrant-token-celo-only.json"
check "settlement-moving token recovered on both chains" 1 "recovers other_token, a token that moves the settlement balance" -- "$fx/settlement-mover-both-chains.json"
check "settlement-moving token on stellar expecting TRANSFER_FAILED" 0 "every check passed" -- "$fx/settlement-mover-stellar-transfer-failed.json"

# Fee fields.
check "numeric fee" 1 "args.req.fee: 5 is not of type 'string'" -- "$fx/numeric-fee.json"
check "max_fee_bps above 100 is still representable" 0 "every check passed" -- "$fx/max-fee-bps-above-100.json"
check "fee_address used as a caller" 0 "every check passed" -- "$fx/fee-address-caller.json"
check "mandate used as a caller" 1 "caller mandate can never make a call" -- "$fx/mandate-caller.json"

# File hygiene and step order.
check "fraction in a count" 1 "1.5 is not an integer" -- "$fx/fraction.json"
check "repeated key" 1 "the key \"fn\" appears twice in one object" -- "$fx/repeated-key.json"
cp "$fx/base.json" "$work/copy.json"
check "duplicate vector ID across files" 1 "the ID fixture_base is also used in" -- "$fx/base.json" "$work/copy.json"
check "call before the mandate exists" 1 "calls pause before the mandate exists" -- "$fx/call-before-mandate.json"
check "time moving back" 1 "set_time moves time back from 1767225600 to 1767225599" -- "$fx/time-moves-back.json"
check "wrong format" 1 "'ujira-mandate-vectors/1' was expected" "format must be ujira-mandate-vectors/1" -- "$fx/wrong-format.json"

# Coverage, against the small spec: three live MANDATE rules, one retired
# rule and two CHAINOPS rules, none of which any vector cites.
cov=(--spec "$fx/spec")
check "require-all passes when every rule has both" 0 \
  "each of 3 MANDATE rule(s) in the spec has a passing and a failing vector" \
  "1 retired rule(s) and 2 CHAINOPS rule(s) excluded" -- --require-all "${cov[@]}" "$fx/coverage/complete.json"
check "require-all missing failing vector" 1 "coverage: MANDATE-FEE-003 has no failing vector" -- --require-all "${cov[@]}" "$fx/coverage/fee-never-refused.json"
check "areas filter leaves out the areas it does not name" 0 "each of 2 MANDATE rule(s) in PAUSE has a passing and a failing vector" -- --areas PAUSE "${cov[@]}" "$fx/coverage/fee-never-refused.json"
check "areas filter finds the gap in its area" 1 "coverage: MANDATE-FEE-003 has no failing vector" -- --areas FEE "${cov[@]}" "$fx/coverage/fee-never-refused.json"
check "CHAINOPS rules never count as uncovered" 0 "each of 1 MANDATE rule(s) in FEE has a passing and a failing vector" -- --areas FEE "${cov[@]}" "$fx/coverage/complete.json"
check "areas naming an area with no MANDATE rule" 2 "--areas names SWEEP, which has no MANDATE rule" -- --areas SWEEP "${cov[@]}" "$fx/coverage/complete.json"
check "require-all and areas together" 2 "cannot be combined" -- --require-all --areas PAUSE "${cov[@]}" "$fx/coverage/complete.json"
check "require-all on the real spec names uncovered rules" 1 "coverage: MANDATE-FEE-003 has no passing and no failing vector" -- --require-all

# The pinned check-jsonschema, and bad usage.
code=0
out=$(CHECK_JSONSCHEMA="$work/no-such-tool" "$script" 2>&1) || code=$?
if [ "$code" -eq 2 ] && printf '%s\n' "$out" | grep -qF "no-such-tool not found"; then pass "missing check-jsonschema (exits 2 as intended)"; else fail "missing check-jsonschema: exit $code, want 2" "$out"; fi
printf '#!/bin/sh\necho "check-jsonschema, version 0.1.0"\n' >"$work/old-check-jsonschema"
chmod +x "$work/old-check-jsonschema"
code=0
out=$(CHECK_JSONSCHEMA="$work/old-check-jsonschema" "$script" 2>&1) || code=$?
if [ "$code" -eq 2 ] && printf '%s\n' "$out" | grep -qF "is version 0.1.0, not the pinned"; then pass "wrong check-jsonschema version (exits 2 as intended)"; else fail "wrong check-jsonschema version: exit $code, want 2" "$out"; fi
check "unknown option" 2 "unknown option --frobnicate" -- --frobnicate
check "unreadable vector file" 2 "cannot read" -- "$work/no-such-file.json"

# The schema comes from the checkout the script sits in, so a copy with a
# broken schema shows that an invalid JSON Schema fails the check.
mkdir -p "$work/tree/scripts" "$work/tree/spec/vectors"
cp "$script" "$here/check-vectors.py" "$here/check-vectors.requirements.txt" "$work/tree/scripts/"
cp "$root/spec/MANDATE.md" "$root/spec/errors.json" "$root/spec/EVENTS.md" "$root/spec/retired-ids.txt" "$work/tree/spec/"
cp "$root/spec/vectors/example.json" "$work/tree/spec/vectors/"
printf '{ "type": "not a type" }\n' >"$work/tree/spec/vectors/schema.json"
code=0
out=$("$work/tree/scripts/check-vectors.sh" 2>&1) || code=$?
if [ "$code" -eq 1 ] && printf '%s\n' "$out" | grep -qF "schemafile was not valid"; then pass "invalid JSON Schema (refused as intended)"; else fail "invalid JSON Schema: exit $code, want 1" "$out"; fi

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
