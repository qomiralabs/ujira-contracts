#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Self test for attestation-message.py: the worked example it computes must
# match spec/ENCODING.md byte for byte, its address encoding must match the
# Address::to_xdr bytes recorded in the attestation parity fixture, every byte
# count and length in spec/ENCODING.md must equal what --derive computes from
# the layout, and bad inputs must be refused.
#
# The tamper cases matter most: each one changes a figure in a copy of
# ENCODING.md and requires --check-doc to catch it. A figure that cannot be
# got wrong without a test failing is a figure nobody has to recompute by hand.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
script="$root/scripts/attestation-message.py"
encoding="$root/spec/ENCODING.md"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

passed=0
failures=0
ok() { echo "ok: $1"; passed=$((passed + 1)); }
fail() { echo "FAIL: $1"; failures=$((failures + 1)); }

out=$("$script" --example)
for key in fields sha256 message; do
  value=$(printf '%s\n' "$out" | awk -v k="$key" '$1 == k { print $2 }')
  if [ -n "$value" ] && grep -qF "\`$value\`" "$encoding"; then ok "example $key matches ENCODING.md"; else fail "example $key $value is not in ENCODING.md"; fi
done
if printf '%s\n' "$out" | grep -qx 'length  47'; then ok "example message is 47 bytes"; else fail "example message is not 47 bytes"; fi

# Address::to_xdr bytes as the Stellar SDK produced them for the fixture.
check_xdr() {
  local name=$1 strkey=$2 want=$3 got
  got=$("$script" --xdr "$strkey")
  if [ "$got" = "$want" ]; then ok "$name"; else fail "$name: got $got, want $want"; fi
}
check_xdr "contract address bytes" CDUKSTCG3XKTPTHMNRJZIW23J3T5DKHASDPQ3CIRKQ4YJH3PDOFFULZA \
  0000001200000001e8a94c46ddd537ccec6c53945b5b4ee7d1a8e090df0d89115439849f6f1b8a5a
check_xdr "account address bytes" GDTYAUMMX45E2SU2Q7JRJHCXDIYTEUAXVBHZ7DVCVXZGMAKMWFL7SOY4 \
  000000120000000000000000e780518cbf3a4d4a9a87d3149c571a31325017a84f9f8ea2adf266014cb157f9
check_xdr "muxed address gives its base account and tag" MDTYAUMMX45E2SU2Q7JRJHCXDIYTEUAXVBHZ7DVCVXZGMAKMWFL7SAAAAAAAAAAAA6AYY \
  "000000120000000000000000e780518cbf3a4d4a9a87d3149c571a31325017a84f9f8ea2adf266014cb157f9 tag 7"
check_xdr "muxed address with the largest tag" MDTYAUMMX45E2SU2Q7JRJHCXDIYTEUAXVBHZ7DVCVXZGMAKMWFL7T7777777777776DVU \
  "000000120000000000000000e780518cbf3a4d4a9a87d3149c571a31325017a84f9f8ea2adf266014cb157f9 tag 18446744073709551615"

code=0
"$script" --xdr GDTYAUMMX45E2SU2Q7JRJHCXDIYTEUAXVBHZ7DVCVXZGMAKMWFL7SOY5 >/dev/null 2>&1 || code=$?
if [ "$code" -eq 1 ]; then ok "bad checksum refused"; else fail "bad checksum: exit $code, want 1"; fi
code=0
"$script" --frobnicate >/dev/null 2>&1 || code=$?
if [ "$code" -eq 2 ]; then ok "bad usage exits 2"; else fail "bad usage: exit $code, want 2"; fi

# Every figure in ENCODING.md is derived from the layout by --derive, and
# --check-doc compares the two. The derivation is printed so the numbers in
# the file can be read against their source.
echo
"$script" --derive
echo

code=0
"$script" --check-doc "$encoding" >"$work/check.out" 2>&1 || code=$?
if [ "$code" -eq 0 ]; then ok "every figure in ENCODING.md matches the derivation"; else fail "check-doc on the real file: exit $code"; sed 's/^/    /' "$work/check.out"; fi

# tamper <name> <sed expression> <text the complaint must contain>
tamper() {
  local name=$1 expr=$2 want=$3 copy="$work/tampered.md" out code=0
  sed "$expr" "$encoding" >"$copy"
  if cmp -s "$copy" "$encoding"; then fail "$name: the tamper changed nothing, so the case proves nothing"; return; fi
  out=$("$script" --check-doc "$copy" 2>&1) || code=$?
  if [ "$code" -eq 1 ] && printf '%s\n' "$out" | grep -qF -- "$want"; then
    ok "$name"
  else
    fail "$name: exit $code, want 1 with '$want'"
    printf '%s\n' "$out" | sed 's/^/    /'
  fi
}

tamper "a wrong signature length is caught"    's/exactly 64 bytes/exactly 65 bytes/'                         'signature'
tamper "a wrong attester key length is caught" 's/a 32-byte ed25519 public key/a 31-byte ed25519 public key/'  'attester key'
tamper "a wrong address table cell is caught"  's/| 44 |/| 45 |/'                                              'account address'
tamper "a wrong contract length is caught"     's/(40 bytes for a contract/(41 bytes for a contract/'          'contract_xdr'
tamper "a wrong fields length is caught"       's/(207 bytes)/(208 bytes)/'                          'fields'
tamper "a wrong message length is caught"      's/= 47 bytes/= 48 bytes/'                                      'message'
tamper "a wrong prefix length is caught"       's/the 15 ASCII bytes/the 16 ASCII bytes/'                      'prefix'
tamper "a wrong hash length is caught"         's/the 32-byte SHA-256/the 31-byte SHA-256/'                    'SHA-256'
tamper "a wrong integer width is caught"       's/of 1, 2, 4 and 8 bytes/of 1, 2, 4 and 9 bytes/'              'u64be'
tamper "a wrong UUID length is caught"         's/its 16 bytes/its 17 bytes/'                                  'UUID'
tamper "a wrong bracket width is caught"       's/network_id\[32\]/network_id[31]/'                            'network_id'
tamper "an unknown bracket field is named"     's/network_id\[32\]/widget_id[32]/'                             'add it to BRACKET_WIDTHS'
tamper "a wrong leg_key length is caught"      's/is 33 bytes/is 34 bytes/'                          'leg_key'
tamper "a wrong ID length is caught"           's/are 32 bytes/are 31 bytes/'                'ID'
tamper "a wrong leg index width is caught"     's/and 1 byte of leg index/and 2 bytes of leg index/'          'u8'
tamper "a wrong length prefix width is caught" 's/a 1 byte length prefix/a 2 byte length prefix/'             'u8'
tamper "a figure that is removed is caught"    '/The signature is exactly/d'                                   'no figure matched'

# A length given in bits is an uncovered figure, so the convention's bits
# half is enforced rather than only stated.
copy="$work/bits.md"
{ cat "$encoding"; echo; echo "- The nonce is 96 bits long."; } >"$copy"
code=0
out=$("$script" --check-doc "$copy" 2>&1) || code=$?
if [ "$code" -eq 1 ] && printf '%s\n' "$out" | grep -qF 'not covered'; then ok "a length given in bits is caught"; else fail "bits figure: exit $code, want 1 with 'not covered'"; printf '%s\n' "$out" | sed 's/^/    /'; fi

# The bytes guard takes a space or a hyphen, so the bits guard must too: a
# hyphenated bit width is the form a byte layout document would use.
copy="$work/hyphen-bits.md"
{ cat "$encoding"; echo; echo "- The tag is a 64-bit unsigned integer."; } >"$copy"
code=0
out=$("$script" --check-doc "$copy" 2>&1) || code=$?
if [ "$code" -eq 1 ] && printf '%s\n' "$out" | grep -qF 'not covered'; then ok "a hyphenated bit width is caught"; else fail "hyphenated bits: exit $code, want 1 with 'not covered'"; printf '%s\n' "$out" | sed 's/^/    /'; fi

# A figure no check covers must fail too, so a new one cannot be added without
# a derivation behind it.
copy="$work/extra.md"
{ cat "$encoding"; echo; echo "- The widget header is 99 bytes long."; } >"$copy"
code=0
out=$("$script" --check-doc "$copy" 2>&1) || code=$?
if [ "$code" -eq 1 ] && printf '%s\n' "$out" | grep -qF 'not covered'; then ok "an uncovered figure is caught"; else fail "uncovered figure: exit $code, want 1 with 'not covered'"; printf '%s\n' "$out" | sed 's/^/    /'; fi

# The worked example must be the fixture's valid_untagged case, so the file and
# the generated fixture can never drift apart.
fixture="$root/spikes/attestation-parity/fixture.json"
if [ -f "$fixture" ]; then
  if python3 - "$fixture" "$encoding" <<'PYCHECK'
import json, sys

fixture, encoding = sys.argv[1], sys.argv[2]
case = next(c for c in json.load(open(fixture))["cases"] if c["name"] == "valid_untagged")
att = case["attestations"][0]
doc = open(encoding).read()
wanted = {
    "fields": att["checked"]["fields"],
    "fields_sha256": att["checked"]["fields_sha256"],
    "message": att["checked"]["message"],
    "attester public key": case["attester_public_key"],
    "signature": att["signature"],
}
missing = [name for name, value in wanted.items() if "`%s`" % value[2:] not in doc]
if missing:
    print("not in ENCODING.md: " + ", ".join(missing), file=sys.stderr)
    sys.exit(1)
PYCHECK
  then ok "the worked example is the fixture's valid_untagged case"; else fail "the worked example does not match the fixture"; fi
else
  fail "the attestation parity fixture is missing, so the example cannot be cross-checked"
fi

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
