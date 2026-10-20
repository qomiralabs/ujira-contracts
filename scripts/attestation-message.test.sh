#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Self test for attestation-message.py: the worked example it computes must
# match spec/ENCODING.md byte for byte, its address encoding must match the
# Address::to_xdr bytes recorded in the attestation parity fixture, and bad
# inputs must be refused.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
script="$root/scripts/attestation-message.py"
encoding="$root/spec/ENCODING.md"

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

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
