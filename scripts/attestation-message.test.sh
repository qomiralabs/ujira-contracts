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
# The tamper cases matter most: each one changes a copy of ENCODING.md and
# requires --check-doc to catch it, naming what it refused. A figure or a layout
# that cannot be got wrong without a test failing is one nobody has to recompute
# by hand.
# The backticks in the single quoted patterns below are literal, from the file.
# shellcheck disable=SC2016
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
script="$root/scripts/attestation-message.py"
encoding="$root/spec/ENCODING.md"
exemptions="$root/scripts/encoding-figure-exemptions.json"

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

# The worked tenant_tag, recomputed from its inputs, row by row.
out=$("$script" --tenant-tag-example)
for key in tenant_ref mandate_xdr tenant_tag; do
  value=$(printf '%s\n' "$out" | awk -v k="$key" '$1 == k { print $2 }')
  if [ -n "$value" ] && grep -qF "| \`$key\` | \`$value\` |" "$encoding"; then ok "tenant tag example $key matches ENCODING.md"; else fail "tenant tag example $key $value is not in ENCODING.md"; fi
done

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
cat "$work/check.out"
if grep -qE '^ENCODING\.md: [0-9]+ digit run\(s\): [0-9]+ exempt by construction, [0-9]+ derived, [0-9]+ exempt by name, 0 unexplained$' "$work/check.out"; then
  ok "the counts are printed and nothing is unexplained"
else
  fail "the digit run counts are missing or something is unexplained"
fi

# refused <name> <copy> <text the complaint must contain>...: the copy must exit 1
# and the output must hold every text, so each case names what it refused.
refused() {
  local name=$1 copy=$2 out code=0 needle
  shift 2
  out=$("$script" --check-doc "$copy" 2>&1) || code=$?
  if [ "$code" -ne 1 ]; then
    fail "$name: exit $code, want 1"
    printf '%s\n' "$out" | sed 's/^/    /'
    return
  fi
  for needle in "$@"; do
    if ! grep -qF -- "$needle" <<<"$out"; then
      fail "$name: the complaint does not contain '$needle'"
      printf '%s\n' "$out" | sed 's/^/    /'
      return
    fi
  done
  ok "$name"
}

# tamper <name> <sed expression> <text the complaint must contain>...
tamper() {
  local name=$1 expr=$2 copy="$work/tampered.md"
  shift 2
  sed "$expr" "$encoding" >"$copy"
  if cmp -s "$copy" "$encoding"; then fail "$name: the tamper changed nothing, so the case proves nothing"; return; fi
  refused "$name" "$copy" "$@"
}

# line_with <fixed text>: the line number of its first match in ENCODING.md
line_with() {
  grep -nF -- "$1" "$encoding" | head -n 1 | cut -d: -f1
}

# added <name> <line added at the end> <text the complaint must contain>...
added() {
  local name=$1 line=$2 copy="$work/added.md"
  shift 2
  { cat "$encoding"; echo; printf '%s\n' "$line"; } >"$copy"
  refused "$name" "$copy" "$@"
}

tail_line=$(($(wc -l <"$encoding") + 2))

tamper "a wrong signature length is caught"    's/exactly 64 bytes/exactly 65 bytes/'                         'the signature length says 65, the layout derives 64'
tamper "a wrong attester key length is caught" 's/a 32-byte ed25519 public key/a 31-byte ed25519 public key/'  'the attester key length says 31, the layout derives 32'
tamper "a wrong address table cell is caught"  's/| 44 |/| 45 |/'                                              'the account address length in the table says 45, the layout derives 44'
tamper "a wrong contract length is caught"     's/(40 bytes for a contract/(41 bytes for a contract/'          'the contract and account address lengths says 41, the layout derives 40'
tamper "a wrong fields length is caught"       's/(207 bytes)/(208 bytes)/'                                    "the worked example's fields length says 208, the layout derives 207"
tamper "a wrong message length is caught"      's/= 47 bytes/= 48 bytes/'                                      'the message, prefix and SHA-256 lengths says 48, the layout derives 47'
tamper "a wrong prefix length is caught"       's/the 15 ASCII bytes/the 16 ASCII bytes/'                      'the message, prefix and SHA-256 lengths says 16, the layout derives 15'
tamper "a wrong hash length is caught"         's/the 32-byte SHA-256/the 31-byte SHA-256/'                    'the message, prefix and SHA-256 lengths says 31, the layout derives 32'
tamper "a wrong integer width is caught"       's/of 1, 2, 4 and 8 bytes/of 1, 2, 4 and 9 bytes/'              'the integer widths says 9, the layout derives 8 (u64be)'
tamper "a wrong UUID length is caught"         's/its 16 bytes/its 17 bytes/'                                  'the UUID length says 17, the layout derives 16'
tamper "a wrong bracket width is caught"       's/network_id\[32\]/network_id[31]/'                            'the network_id width says 31, the layout derives 32'
tamper "an unknown bracket field is named"     's/network_id\[32\]/widget_id[32]/'                             'the width of widget_id[32] is not covered by any derivation; add it to BRACKET_WIDTHS'
tamper "a wrong leg_key length is caught"      's/is 33 bytes/is 34 bytes/'                                    'the leg_key length says 34, the layout derives 33'
tamper "a wrong ID length is caught"           's/are 32 bytes/are 31 bytes/'                                  'the ID length says 31, the layout derives 32'
tamper "a wrong leg index width is caught"     's/and 1 byte of leg index/and 2 bytes of leg index/'           'the leg index width says 2, the layout derives 1'
tamper "a wrong length prefix width is caught" 's/a 1 byte length prefix/a 2 byte length prefix/'              'the address length prefix width says 2, the layout derives 1'
tamper "a figure that is removed is caught"    '/The signature is exactly/d'                                   'no figure matched the signature length'

# The layout that decides what gets signed, evaluated rather than counted. These
# all passed before the layout was pinned, with every figure reported as equal.
line=$(line_with 'message = "UJIRA/ATTEST/V2"')
tamper "the prefix literal in the layout line is pinned" 's|^message = "UJIRA/ATTEST/V2"|message = "UJIRA/ATTEST/V3"|' \
  "tampered.md:$line: the message layout does not build the bytes this script builds" 'UJIRA/ATTEST/V3'
line=$(line_with '| Account `G` |')
tamper "the address discriminant is round tripped"  's/u32be(18)/u32be(19)/g' \
  "tampered.md:$line: the Account \`G\` address does not encode as Address::to_xdr does" 'u32be(19)'
tamper "the address discriminant in the prose is pinned" 's/`SCV_ADDRESS` (18)/`SCV_ADDRESS` (19)/' \
  'the address discriminant in the prose says 19, the layout derives 18'
tamper "the account type value is round tripped" 's/u32be(18) ‖ u32be(0) ‖ u32be(0)/u32be(18) ‖ u32be(2) ‖ u32be(0)/' \
  'the Account `G` address does not encode as Address::to_xdr does' 'u32be(2)'
tamper "the contract type value is round tripped" 's/u32be(18) ‖ u32be(1) ‖ contract_id/u32be(18) ‖ u32be(3) ‖ contract_id/' \
  'the Contract `C` address does not encode as Address::to_xdr does' 'u32be(3)'
line=$(line_with 'fields  = network_id')
tamper "swapping recipient_ref with country_policy_hash is caught" \
  's/^\( *‖ \)recipient_ref\[32\]$/\1TEMP/; s/^\( *‖ \)country_policy_hash\[32\]$/\1recipient_ref[32]/; s/^\( *‖ \)TEMP$/\1country_policy_hash[32]/' \
  "tampered.md:$line: the fields layout does not build the bytes this script builds"
line=$(line_with 'payout_id     = SHA-256')
tamper "a u16be length prefix changed to u32be is caught" 's/u16be(len(idempotency_key))/u32be(len(idempotency_key))/' \
  "tampered.md:$line: the payout_id layout does not build the bytes this script builds" 'u32be(len(idempotency_key))'
tamper "the policy epoch changed to u32be is caught" 's/u64be(policy_epoch)/u32be(policy_epoch)/' \
  'the fields layout does not build the bytes this script builds' 'u32be(policy_epoch)'
tamper "the payout domain tag is pinned" 's|ujira/payout/v1|ujira/payouts/v1|' \
  'the payout_id layout does not build the bytes this script builds' 'ujira/payouts/v1'
tamper "the recipient domain tag is pinned" 's|ujira/recipient/v1|ujira/recipients/v1|' 'the recipient_ref layout does not build'
tamper "the task domain tag is pinned" 's|ujira/task/v1|ujira/tasks/v1|' 'the task_ref layout does not build'
tamper "the tenant domain tag is pinned" 's|"ujira/tenant/v1"|"ujira/tenants/v1"|' 'the tenant_ref layout does not build'
tamper "the tenant tag domain tag is pinned" 's|ujira/tenant-tag/v1|ujira/tenant-tags/v1|' 'the tenant_tag layout does not build'
tamper "swapping the tag and has_tag is caught" 's/u8(has_tag) ‖ u64be(tag)/u64be(tag) ‖ u8(has_tag)/' 'the fields layout does not build'
tamper "swapping policy_epoch and valid_until is caught" \
  's/u64be(policy_epoch)$/TEMP/; s/u64be(valid_until)$/u64be(policy_epoch)/; s/TEMP$/u64be(valid_until)/' 'the fields layout does not build'
tamper "a layout that is removed is caught" '/^leg_key       = /d' 'no layout defines leg_key'
tamper "a layout that is renamed is caught" 's/^leg_key       = /leg_keys      = /' '"leg_keys" is not a layout this script builds' 'no layout defines leg_key'
tamper "a layout written twice is caught" 's/^\(leg_key       = .*\)$/\1\n\1/' 'the leg_key layout is written twice'
tamper "an address row that is removed is caught" '/^| Contract `C` |/d' 'no address table row for Contract `C`'
tamper "a changed width inside a fenced layout block fails" 's/recipient_ref\[32\]/recipient_ref[33]/' \
  'the recipient_ref width says 33, the layout derives 32' 'recipient_ref[33]'

# A fence closes on its own line, or everything after it would look like code.
copy="$work/open-fence.md"
{ cat "$encoding"; echo; echo '```'; echo "- The widget is 99 bytes."; } >"$copy"
refused "a fence that never closes is caught" "$copy" 'a code fence never closes'

# The asymmetry: the derivation patterns stay case sensitive while the sweep is
# blind to case, and the comment over DOC_CHECKS says why. The shipped script must
# refuse a correct figure with a capitalised unit. The variant with case
# insensitive patterns must accept it, which is what proves this case can tell the
# two designs apart: if someone makes the patterns case insensitive, the first half
# fails.
copy="$work/capital-correct.md"
sed 's/A UUID is its 16 bytes/A UUID is its 16 BYTES/' "$encoding" >"$copy"
if cmp -s "$copy" "$encoding"; then fail "the asymmetry case: the rewording changed nothing"; fi
refused "derivation patterns stay case sensitive: a correct figure with a capitalised unit fails" "$copy" \
  'no figure matched the UUID length' 'the number 16 in "- A UUID is its 16 BYTES'
variant="$work/case-insensitive.py"
sed 's/re\.finditer(pattern, text, re\.M)/re.finditer(pattern, text, re.M | re.I)/' "$script" >"$variant"
if cmp -s "$variant" "$script"; then fail "the asymmetry case: the variant is the shipped script, so the case proves nothing"; fi
code=0
out=$(python3 "$variant" --check-doc "$copy" --exemptions "$exemptions" 2>&1) || code=$?
if [ "$code" -eq 0 ]; then
  ok "case insensitive derivation patterns would wave that figure through, so the case above is what holds them"
else
  fail "the case insensitive variant should accept the capitalised figure, exit $code"
  printf '%s\n' "$out" | sed 's/^/    /'
fi

# Forms that used to pass or be misreported. Hex was silent for half its alphabet:
# 0xFF and 0xff had no digit beside the unit, so nothing looked at them.
added "0x20 bytes fails and says what it read" "- The header is 0x20 bytes." \
  "added.md:$tail_line: the figure \"0x20 bytes\" reads as the hex literal 0x20, which is 32; write a length in decimal digits"
added "0xFF bytes fails and says what it read" "- The header is 0xFF bytes." \
  "added.md:$tail_line: the figure \"0xFF bytes\" reads as the hex literal 0xFF, which is 255; write a length in decimal digits"
added "0xff bytes fails and says what it read" "- The header is 0xff bytes." \
  "added.md:$tail_line: the figure \"0xff bytes\" reads as the hex literal 0xff, which is 255; write a length in decimal digits"
added "1,024 bytes fails and says what it read" "- The header is 1,024 bytes." \
  "added.md:$tail_line: the figure \"1,024 bytes\" reads as 1024 with a thousands separator; write a length as plain digits"

# A figure with no unit, wherever it sits.
added "a bare 48 in prose fails" "- The header holds 48 of them." \
  "added.md:$tail_line: the number 48 in \"- The header holds 48 of them.\" is not covered"
added "a bare 48 in a heading fails" "## Section 48" \
  "added.md:$tail_line: the number 48 in \"## Section 48\" is not covered"
line=$(line_with '| `policy_epoch` | 0 |')
tamper "a bare 48 in a table cell fails" 's/^| `policy_epoch` | 0 |$/&\n| `widget` | 48 |/' \
  "tampered.md:$((line + 1)): the number 48 in \"| \`widget\` | 48 |\" is not covered"
line=$(line_with '‖ u8(has_tag) ‖ u64be(tag)')
tamper "a bare 48 as a term in the fenced layout block fails" 's/‖ u8(has_tag) ‖ u64be(tag)$/& ‖ 48/' \
  'the fields layout: cannot read the term "48"'
line=$(line_with 'message = "UJIRA/ATTEST/V2"')
tamper "a bare 48 on its own line in the fenced layout block fails" 's/^message = "UJIRA.*$/&\n48/' \
  "tampered.md:$((line + 1)): cannot read \"48\" in a layout block"

# Nothing is blessed by its value: a small number is where an off by one hides.
added "a bare 0 with no unit fails" "- The flag starts at 0 here." "the number 0 in \"- The flag starts at 0 here.\" is not covered"
added "a bare 1 with no unit fails" "- The flag ends at 1 here." "the number 1 in \"- The flag ends at 1 here.\" is not covered"
added "a length run into its unit fails" "- The header is 32bytes." "the number 32 in \"- The header is 32bytes.\" is not covered"
added "a plain long number is a number, not a hex run" "- The header is 12345678." "the number 12345678 in"

# The three forms that were missed. Each was found in the review of #104, by a
# different reviewer, and each is why the check stopped listing forms.
added "a length spelled in words is caught (found in the first review of #104)" "- The nonce is thirty two bytes long." \
  "added.md:$tail_line: the length \"thirty two bytes\" is spelled in words"
added "a length spelled with a hyphen is caught" "- The nonce is a sixty-four-byte value." "the length \"sixty-four-byte\" is spelled in words"
added "a bit width with a hyphen is caught (found in the second review of #104)" "- The tag is a 64-bit unsigned integer." \
  "added.md:$tail_line: the number 64 in \"- The tag is a 64-bit unsigned integer.\" is not covered"
added "a length with a capital letter is caught (found in the third review of #104)" "- The address is 99 Bytes." \
  "added.md:$tail_line: the number 99 in \"- The address is 99 Bytes.\" is not covered"
added "a length given in bits is caught" "- The nonce is 96 bits long." "the number 96 in \"- The nonce is 96 bits long.\" is not covered"
added "an uncovered figure is caught" "- The widget header is 99 bytes long." "the number 99 in \"- The widget header is 99 bytes long.\" is not covered"

# Exempt by construction, never by value.
copy="$work/identifiers.md"
{ cat "$encoding"; echo; echo "- The key is ed25519, hashed with SHA-256, as in RFC 9562, and \`123\` is code."; } >"$copy"
code=0
out=$("$script" --check-doc "$copy" 2>&1) || code=$?
if [ "$code" -eq 0 ]; then ok "ed25519, SHA-256, RFC 9562 and inline code pass as identifiers"; else fail "identifiers: exit $code"; printf '%s\n' "$out" | sed 's/^/    /'; fi

# The table guard reads through the header, and reports what it cannot read.
line=$(line_with '| Account `G` |')
tamper "a fourth column" 's/^\(| Account `G` .*\) | 44 |$/\1 | 44 | extra |/' \
  "tampered.md:$line: the table row \"| Account \`G\`" "has 4 cells under a header of 3, so it is not read"
tamper "the number in another cell" 's/^| Account `G` | \(.*\) | 44 |$/| Account `G` | 44 \1 | bytes |/' \
  "tampered.md:$line: the table row \"| Account \`G\`" "has no number in its Length column"
tamper "a table row with no derivation" 's/^| Account `G` | \(.*\) | 44 |$/&\n| Muxed `M` | `x` | 52 |/' \
  'the table row "| Muxed `M` | `x` | 52 |" has no derivation; add it to TABLE_LENGTHS'
copy="$work/moved-column.md"
sed -e 's/^| Address | Bytes | Length |$/| Length | Address | Bytes |/' \
  -e 's/^| Account `G` | \(.*\) | 44 |$/| 44 | Account `G` | \1 |/' \
  -e 's/^| Contract `C` | \(.*\) | 40 |$/| 40 | Contract `C` | \1 |/' "$encoding" >"$copy"
code=0
out=$("$script" --check-doc "$copy" 2>&1) || code=$?
if [ "$code" -eq 0 ]; then ok "a moved column is still read through the header"; else fail "a moved column: exit $code"; printf '%s\n' "$out" | sed 's/^/    /'; fi
tamper_moved=$(sed -e 's/^| 44 | /| 45 | /' "$copy")
printf '%s\n' "$tamper_moved" >"$work/moved-wrong.md"
refused "a wrong number in a moved column is caught" "$work/moved-wrong.md" 'the account address length in the table says 45'

# Named exemptions must be worth reading.
exempt_case() {
  local name=$1 entries=$2 want=$3 code=0 out
  printf '%s\n' "$entries" >"$work/exemptions.json"
  out=$("$script" --check-doc "$encoding" --exemptions "$work/exemptions.json" 2>&1) || code=$?
  if [ "$code" -eq 1 ] && grep -qF -- "$want" <<<"$out"; then
    ok "$name"
  else
    fail "$name: exit $code, want 1 with '$want'"
    printf '%s\n' "$out" | sed 's/^/    /'
  fi
}
one_real='{ "name": "the first attempt number", "pattern": "`attempt` starts at (\\d+)", "reason": "A value." }'
exempt_case "an exemption entry without a reason fails" \
  '[{ "name": "no reason given", "pattern": "`attempt` starts at (\\d+)", "reason": "  " }]' \
  '"no reason given" has no reason'
exempt_case "an exemption entry missing its reason key fails" \
  '[{ "name": "no reason key", "pattern": "`attempt` starts at (\\d+)" }]' \
  '"no reason key" must have exactly a name, a pattern and a reason'
exempt_case "an exemption that is a bare value fails" \
  '[{ "name": "just digits", "pattern": "(\\d+)", "reason": "Everything." }]' \
  '"just digits" is a bare value'
exempt_case "an exemption of one value fails" \
  '[{ "name": "just zero", "pattern": "starts at (0)", "reason": "A value." }]' \
  '"just zero" is a bare value'
exempt_case "an exemption that covers nothing fails" \
  "[$one_real, { \"name\": \"stale\", \"pattern\": \"no such words (\\\\d+)\", \"reason\": \"Gone.\" }]" \
  'the exemption "stale" covers no number the sweep needs; remove it'
exempt_case "dropping an exemption brings its number back" "[$one_real]" 'the number 2 in "## Attestation message (version 2)" is not covered'

# Every committed exemption has a reason a reviewer can read, and the file holds no
# step that strips fences before the bracket sweep.
if python3 - "$exemptions" <<'PYCHECK'
import json, sys
entries = json.load(open(sys.argv[1]))
assert entries and all(len(e["name"]) > 8 and len(e["reason"]) > 20 for e in entries)
PYCHECK
then ok "every committed exemption has a name and a reason"; else fail "a committed exemption has a thin name or reason"; fi

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
