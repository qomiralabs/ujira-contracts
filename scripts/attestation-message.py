#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Builds the version 2 attestation message from its inputs, exactly as
# spec/ENCODING.md lays it out, so the documented bytes can be recomputed
# without the Stellar SDK. Addresses are decoded from their strkeys here and
# encoded as Address::to_xdr bytes by hand.
#
# Standard library only, no dependencies.
#
# Usage:
#   scripts/attestation-message.py --example
#       print the field bytes, their SHA-256 and the 47-byte message of the
#       worked example in spec/ENCODING.md
#   scripts/attestation-message.py --tenant-tag-example
#       recompute the worked tenant_ref and tenant_tag in spec/ENCODING.md
#   scripts/attestation-message.py --xdr <strkey>
#       print the Address::to_xdr bytes of a G, C or M address (an M address
#       gives its base G address and its id)
#   scripts/attestation-message.py --derive
#       print every byte count and length in spec/ENCODING.md, each computed
#       from the layout rather than quoted, with how it was computed
#   scripts/attestation-message.py --check-doc spec/ENCODING.md [--exemptions FILE]
#       check the file in three steps, printing the digit run counts:
#       1. every layout block and address table expression is evaluated and must
#          build the bytes this script builds, so the literals, field order,
#          integer widths and discriminants are checked and not only counted;
#       2. every figure paired with a derivation must equal it;
#       3. every other digit run must be exempt by construction or named in
#          scripts/encoding-figure-exemptions.json with a reason. Anything else
#          fails with its line and text, whatever form it is written in.
#   scripts/attestation-message.py --check-constants spec/constants.json
#       check the attestation lengths in the constants file against --derive
# Exit codes: 0 done, 1 an input is invalid or a figure disagrees, 2 bad usage.

import base64
import hashlib
import json
import os
import re
import sys
import uuid

PREFIX = b"UJIRA/ATTEST/V2"
U64_MAX = (1 << 64) - 1
EXEMPTIONS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "encoding-figure-exemptions.json")
CONCAT = "‖"

# Strkey version bytes (SEP-23).
VERSION_ACCOUNT = 6 << 3
VERSION_CONTRACT = 2 << 3
VERSION_MUXED = 12 << 3

# XDR discriminants: ScValType SCV_ADDRESS, ScAddressType, PublicKeyType.
SCV_ADDRESS = 18
SC_ADDRESS_ACCOUNT = 0
SC_ADDRESS_CONTRACT = 1
PUBLIC_KEY_ED25519 = 0

EXAMPLE = {
    "network_passphrase": "Test SDF Network ; September 2015",
    "mandate": "CDUKSTCG3XKTPTHMNRJZIW23J3T5DKHASDPQ3CIRKQ4YJH3PDOFFULZA",
    "recipient_ref": "187204dacf1b6ae9830406f0405b3710d69d86df034cf3fe3753e7b460c8e97d",
    "country_policy_hash": "343fb1d75ee948735be1a836a29a40ecd0612986ab9bd1183f8271ccca830415",
    "policy_epoch": 0,
    "valid_until": 1767229200,
    "destination": "GDTYAUMMX45E2SU2Q7JRJHCXDIYTEUAXVBHZ7DVCVXZGMAKMWFL7SOY4",
    # The attester key and signature of the same case. Both are public by
    # construction: the key's ed25519 seed is the SHA-256 of the ASCII label
    # "UJIRA/TEST-ONLY/S18/attester-ed25519". Their lengths are measured from
    # these bytes rather than written down.
    "attester_public_key": "421adb50c10fe73fb1fc0d4211a16911a188bb001eaf0391b479316026141d48",
    "signature": "043dcca5e9a6dfd41320f9ecf7b4778bf00ff706ac26d865f4c1ff6366a49e04"
                 "97e63ef43e2a0e6404218231d10d11aa030f954d9eb799ff92f0328e1cae2208",
}


# The worked tenant_tag example: two UUIDs give the tenant_ref, which is
# bound to the attestation example's mandate.
TENANT_EXAMPLE = {
    "principal_uuid": "0b2f3c1e-6a4d-4e8b-9c71-5d2a8f3e4b10",
    "project_uuid": "7c9e2a41-3b8f-4d6c-a1e5-92f04b7d8c33",
}


class InputError(Exception):
    pass


def crc16_xmodem(data):
    crc = 0
    for byte in data:
        crc ^= byte << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    return crc


def decode_strkey(strkey):
    """Returns (version byte, payload) of a strkey, checking its checksum."""
    try:
        raw = base64.b32decode(strkey + "=" * (-len(strkey) % 8))
    except ValueError as err:
        raise InputError("%s is not base32: %s" % (strkey, err))
    if len(raw) < 3:
        raise InputError("%s is too short" % strkey)
    body, checksum = raw[:-2], raw[-2:]
    if crc16_xmodem(body) != int.from_bytes(checksum, "little"):
        raise InputError("%s has a bad checksum" % strkey)
    return body[0], body[1:]


def address_xdr(strkey):
    """Returns (Address::to_xdr bytes of the base address, tag or None)."""
    version, payload = decode_strkey(strkey)
    head = SCV_ADDRESS.to_bytes(4, "big")
    if version == VERSION_ACCOUNT and len(payload) == 32:
        return head + SC_ADDRESS_ACCOUNT.to_bytes(4, "big") + PUBLIC_KEY_ED25519.to_bytes(4, "big") + payload, None
    if version == VERSION_CONTRACT and len(payload) == 32:
        return head + SC_ADDRESS_CONTRACT.to_bytes(4, "big") + payload, None
    if version == VERSION_MUXED and len(payload) == 40:
        key, tag = payload[:32], int.from_bytes(payload[32:], "big")
        return head + SC_ADDRESS_ACCOUNT.to_bytes(4, "big") + PUBLIC_KEY_ED25519.to_bytes(4, "big") + key, tag
    raise InputError("%s is not a G, C or M address" % strkey)


def fixed(hex_text, name, size=32):
    try:
        value = bytes.fromhex(hex_text)
    except ValueError:
        raise InputError("%s is not hex" % name)
    if len(value) != size:
        raise InputError("%s must be %d bytes" % (name, size))
    return value


def u64be(value, name):
    if not 0 <= value <= U64_MAX:
        raise InputError("%s is outside u64" % name)
    return value.to_bytes(8, "big")


def length_prefixed(value, name):
    if len(value) > 255:
        raise InputError("%s is longer than 255 bytes" % name)
    return bytes([len(value)]) + value


def fields(inputs):
    """The attested fields, in the order and widths of spec/ENCODING.md."""
    network_id = hashlib.sha256(inputs["network_passphrase"].encode("utf-8")).digest()
    mandate, mandate_tag = address_xdr(inputs["mandate"])
    if mandate_tag is not None or mandate[4:8] != SC_ADDRESS_CONTRACT.to_bytes(4, "big"):
        raise InputError("the mandate must be a contract (C) address")
    destination, muxed_tag = address_xdr(inputs["destination"])
    tag = inputs.get("tag", muxed_tag)
    if muxed_tag is not None and tag != muxed_tag:
        raise InputError("an M destination already carries its tag")
    return b"".join([
        network_id,
        length_prefixed(mandate, "mandate"),
        fixed(inputs["recipient_ref"], "recipient_ref"),
        fixed(inputs["country_policy_hash"], "country_policy_hash"),
        u64be(inputs["policy_epoch"], "policy_epoch"),
        u64be(inputs["valid_until"], "valid_until"),
        length_prefixed(destination, "destination"),
        bytes([0 if tag is None else 1]),
        u64be(0 if tag is None else tag, "tag"),
    ])


def payout_id(principal_uuid, project_uuid, idempotency_key, attempt):
    return hashlib.sha256(b"ujira/payout/v1" + uuid.UUID(principal_uuid).bytes + uuid.UUID(project_uuid).bytes
                          + len(idempotency_key).to_bytes(2, "big") + idempotency_key
                          + attempt.to_bytes(4, "big")).digest()


def recipient_ref(principal_uuid, recipient_uuid):
    return hashlib.sha256(b"ujira/recipient/v1" + uuid.UUID(principal_uuid).bytes
                          + uuid.UUID(recipient_uuid).bytes).digest()


def task_ref(project_uuid, task_id):
    return hashlib.sha256(b"ujira/task/v1" + uuid.UUID(project_uuid).bytes + task_id).digest()


def tenant_ref(principal_uuid, project_uuid):
    return hashlib.sha256(b"ujira/tenant/v1" + uuid.UUID(principal_uuid).bytes
                          + uuid.UUID(project_uuid).bytes).digest()


def tenant_tag(ref, mandate):
    """The per-mandate tag: the tenant_ref bound to the mandate's address, so
    two mandates of one tenant never share a value on chain."""
    mandate_xdr, _ = address_xdr(mandate)
    return hashlib.sha256(b"ujira/tenant-tag/v1" + fixed(ref.hex(), "tenant_ref") + mandate_xdr).digest()


def message(field_bytes):
    return PREFIX + hashlib.sha256(field_bytes).digest()


def derivations():
    """Every length in spec/ENCODING.md, measured from the layout itself.

    Each entry is (name, bytes, how it was measured). Nothing here is a
    literal taken from the file being checked: every number is the length of
    bytes this script builds.
    """
    account_xdr_bytes, _ = address_xdr(EXAMPLE["destination"])
    contract_xdr_bytes, _ = address_xdr(EXAMPLE["mandate"])
    _, account_payload = decode_strkey(EXAMPLE["destination"])
    _, contract_payload = decode_strkey(EXAMPLE["mandate"])
    example_fields = fields(EXAMPLE)
    # The same case with a contract destination, which is shorter than an account.
    contract_dest_fields = fields(dict(EXAMPLE, destination=EXAMPLE["mandate"]))
    payout_id = hashlib.sha256(b"ujira/payout/v1").digest()
    leg_key = payout_id + bytes([0])
    key = bytes.fromhex(EXAMPLE["attester_public_key"])
    signature = bytes.fromhex(EXAMPLE["signature"])
    return [
        ("u8", len((0).to_bytes(1, "big")), "a u8"),
        ("u16be", len((0).to_bytes(2, "big")), "a u16be"),
        ("u32be", len((0).to_bytes(4, "big")), "a u32be"),
        ("u64be", len(u64be(0, "u64be")), "a u64be, as the layout builds one"),
        ("uuid", len(uuid.UUID(int=0).bytes), "a UUID in RFC 9562 order"),
        ("sha256", len(hashlib.sha256(b"").digest()), "a SHA-256 digest, so every ID too"),
        ("leg_key", len(leg_key), "payout_id and 1 byte of leg index"),
        ("account_xdr", len(account_xdr_bytes), "Address::to_xdr of the example G destination"),
        ("contract_xdr", len(contract_xdr_bytes), "Address::to_xdr of the example C mandate"),
        ("ed25519_key", len(account_payload), "the key inside a G strkey"),
        ("contract_id", len(contract_payload), "the id inside a C strkey"),
        ("prefix", len(PREFIX), "the ASCII message prefix"),
        ("message", len(message(example_fields)), "prefix and SHA-256 of the fields"),
        ("example_fields", len(example_fields), "the worked example's fields, account destination"),
        ("contract_dest_fields", len(contract_dest_fields), "the same fields with a contract destination"),
        ("attester_key", len(key), "the worked example's attester public key"),
        ("signature", len(signature), "the worked example's signature"),
    ]


def scalar_derivations():
    """Numbers in the file that are values rather than lengths, each read back
    from bytes this script builds. Each entry is (name, value, how)."""
    account_xdr_bytes, _ = address_xdr(EXAMPLE["destination"])
    return [
        ("scv_address", int.from_bytes(account_xdr_bytes[:4], "big"),
         "the discriminant at the start of Address::to_xdr"),
    ]


def all_derivations():
    return {name: value for name, value, _ in derivations() + scalar_derivations()}


# Each figure in spec/ENCODING.md, as a pattern whose groups capture the
# documented numbers and the derivation each one must equal. This is the pairing
# layer: it says which derivation a figure answers to. Whether a number is looked
# at all is the digit sweep's job, not this table's.
#
# These patterns are case sensitive and the sweep is blind to case, for two
# different reasons, and both halves matter. The guard has to see a length however
# its unit is written, because the address table's header column is Bytes, so a
# capitalised unit is the natural slip here. The derivation patterns must not
# accept that spelling: a pattern that matched "16 BYTES" would pair the figure,
# compare it, find it right, and report every figure equal to the layout, which
# blesses the form the file forbids. Left case sensitive, the pattern misses it and
# the sweep reports it. scripts/attestation-message.test.sh holds this with a case
# that builds the case insensitive variant and proves the two behave differently.
DOC_CHECKS = [
    ("the integer widths", r"integers of (\d+), (\d+), (\d+) and (\d+) bytes",
     ("u8", "u16be", "u32be", "u64be")),
    ("the ID length", r"`tenant_tag` are (\d+) bytes", ("sha256",)),
    ("the leg_key length", r"`leg_key` is (\d+) bytes", ("leg_key",)),
    ("the leg index width", r"the `payout_id` and (\d+) bytes? of leg index", ("u8",)),
    ("the address length prefix width", r"each after a (\d+)[- ]bytes? length prefix", ("u8",)),
    ("the UUID length", r"A UUID is its (\d+) bytes", ("uuid",)),
    ("the address discriminant in the prose", r"`SCV_ADDRESS` \((\d+)\)", ("scv_address",)),
    ("the contract and account address lengths",
     r"\((\d+) bytes for a contract, (\d+) for an account\)", ("contract_xdr", "account_xdr")),
    ("the policy_epoch width", r"encoded as `u64be`, (\d+) bytes", ("u64be",)),
    ("the message, prefix and SHA-256 lengths",
     r"= (\d+) bytes: the (\d+) ASCII bytes of the prefix and the (\d+)-byte SHA-256",
     ("message", "prefix", "sha256")),
    ("the transaction hash length it cannot equal", r"a (\d+)-byte transaction hash", ("sha256",)),
    ("the attester key length", r"attester key is a (\d+)-byte ed25519 public key", ("attester_key",)),
    ("the attester key length, as a refused zero key", r"(\d+) zero bytes are refused", ("attester_key",)),
    ("the signature length", r"signature is exactly (\d+) bytes", ("signature",)),
    ("the worked example's fields length", r"\| `fields` \((\d+) bytes\) \|", ("example_fields",)),
    ("the worked example's message length", r"\| `message` \((\d+) bytes\) \|", ("message",)),
]

# Fixed widths written as name[n] in the layout blocks.
BRACKET_WIDTHS = {
    "principal_uuid": "uuid",
    "project_uuid": "uuid",
    "recipient_uuid": "uuid",
    "payout_id": "sha256",
    "network_id": "sha256",
    "recipient_ref": "sha256",
    "tenant_ref": "sha256",
    "country_policy_hash": "sha256",
    "ed25519_key": "ed25519_key",
    "contract_id": "contract_id",
}

# Which row of a table with a Length column answers to which derivation. Rows are
# found through the table's header, so moving a column keeps them checked, and a
# row that cannot be read is reported rather than dropped.
TABLE_LENGTHS = {
    "Account `G`": ("the account address length in the table", "account_xdr"),
    "Contract `C`": ("the contract address length in the table", "contract_xdr"),
}

# A number spelled out in front of a unit. The sweep below reads digits, so this
# is the one form of a length it cannot see; it is refused outright.
NUMBER_WORDS = (
    "one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve|thirteen|fourteen|fifteen|sixteen"
    "|seventeen|eighteen|nineteen|twenty|thirty|forty|fifty|sixty|seventy|eighty|ninety|hundred|thousand"
)
SPELLED_LENGTH = re.compile(
    r"\b(?:%s)(?:[- ](?:%s))*[- ](?:ascii )?(?:byte|bit)s?\b" % (NUMBER_WORDS, NUMBER_WORDS), re.I
)
HEX_LITERAL = re.compile(r"\b0[xX][0-9a-fA-F]+(?:[- ](?:bytes?|bits?)\b)?", re.I)
GROUPED_NUMBER = re.compile(r"\b\d{1,3}(?:,\d{3})+(?:[- ](?:bytes?|bits?)\b)?", re.I)
# A hex run needs a hex letter: a long plain number is a number, not a hash.
HEX_RUN = re.compile(r"\b(?=[0-9a-fA-F]*[a-fA-F])[0-9a-fA-F]{8,}\b")


class LayoutError(Exception):
    """A layout expression that cannot be read."""


def line_of(text, offset):
    return text.count("\n", 0, offset) + 1


def line_text(text, offset):
    start = text.rfind("\n", 0, offset) + 1
    end = text.find("\n", offset)
    line = text[start:] if end < 0 else text[start:end]
    return line.strip()[:120]


def code_spans(text):
    """Returns (fence spans, inline code spans, [(line number, line) in fences],
    whether every fence closes)."""
    fences, inline, fenced_lines = [], [], []
    offset, opened = 0, None
    for number, line in enumerate(text.split("\n"), start=1):
        end = offset + len(line)
        if line.startswith("```"):
            if opened is None:
                opened = offset
            else:
                fences.append((opened, end))
                opened = None
        elif opened is not None:
            fenced_lines.append((number, line))
        else:
            for match in re.finditer(r"`[^`]*`", line):
                inline.append((offset + match.start(), offset + match.end()))
        offset = end + 1
    if opened is not None:
        fences.append((opened, len(text)))
    return fences, inline, fenced_lines, opened is None


def inside(spans, offset):
    return any(start <= offset < end for start, end in spans)


def split_terms(expression):
    """Splits on the concatenation mark at the top level only."""
    terms, current, depth, quoted = [], [], 0, False
    for character in expression:
        if character == '"':
            quoted = not quoted
        elif not quoted and character == "(":
            depth += 1
        elif not quoted and character == ")":
            depth -= 1
        elif not quoted and depth == 0 and character == CONCAT:
            terms.append("".join(current).strip())
            current = []
            continue
        current.append(character)
    terms.append("".join(current).strip())
    return terms


def integer_argument(argument, env):
    match = re.fullmatch(r"len\((\w+)\)", argument)
    if match:
        value = env.get(match.group(1))
        if not isinstance(value, bytes):
            raise LayoutError('no bytes for "%s"' % argument)
        return len(value)
    if re.fullmatch(r"\d+", argument):
        return int(argument)
    value = env.get(argument)
    if not isinstance(value, int):
        raise LayoutError('no integer for "%s"' % argument)
    return value


def evaluate(expression, env):
    """The bytes a layout expression builds from env. A term it cannot read is an
    error, never skipped, so a stray figure in a layout fails here."""
    widths = {"u8": 1, "u16be": 2, "u32be": 4, "u64be": 8}
    built = b""
    for term in split_terms(expression):
        integer = re.fullmatch(r"(u8|u16be|u32be|u64be)\((.+)\)", term)
        width = re.fullmatch(r"(\w+)\[(\d+)\]", term)
        if len(term) >= 2 and term[0] == '"' and term[-1] == '"':
            built += term[1:-1].encode("utf-8")
        elif term.startswith("SHA-256(") and term.endswith(")"):
            built += hashlib.sha256(evaluate(term[len("SHA-256("):-1], env)).digest()
        elif integer:
            value = integer_argument(integer.group(2), env)
            try:
                built += value.to_bytes(widths[integer.group(1)], "big")
            except OverflowError:
                raise LayoutError('"%s" does not fit its width' % term)
        elif width:
            value = env.get(width.group(1))
            if not isinstance(value, bytes):
                raise LayoutError('no bytes for "%s"' % term)
            if len(value) != int(width.group(2)):
                raise LayoutError('"%s" is %d bytes in the layout the script builds' % (term, len(value)))
            built += value
        elif re.fullmatch(r"[A-Za-z_]\w*", term) and isinstance(env.get(term), bytes):
            built += env[term]
        else:
            raise LayoutError('cannot read the term "%s"' % term)
    return built


def layout_cases():
    """Returns [(env, {layout name: the bytes this script builds})], with inputs
    that differ from each other, so a swapped pair of fields changes the bytes. The
    attestation has a tagged case because an untagged one is all zeros at the end."""
    principal, project = TENANT_EXAMPLE["principal_uuid"], TENANT_EXAMPLE["project_uuid"]
    recipient = "3f0c7a52-8d1e-4b6a-9f21-6c4e1d7ab905"
    key, attempt, task, leg_index = b"order-2026-11", 3, b"task-42", 2
    ref = tenant_ref(principal, project)
    payout = payout_id(principal, project, key, attempt)
    ids = {
        "payout_id": payout,
        "recipient_ref": recipient_ref(principal, recipient),
        "task_ref": task_ref(project, task),
        "tenant_ref": ref,
        "tenant_tag": tenant_tag(ref, EXAMPLE["mandate"]),
        "leg_key": payout + bytes([leg_index]),
    }
    cases = [(dict(ids, principal_uuid=uuid.UUID(principal).bytes, project_uuid=uuid.UUID(project).bytes,
                   recipient_uuid=uuid.UUID(recipient).bytes, idempotency_key=key, idempotency_key_utf8=key,
                   attempt=attempt, task_id_utf8=task, leg_index=leg_index,
                   mandate_xdr=address_xdr(EXAMPLE["mandate"])[0]), ids)]
    for inputs in (EXAMPLE, dict(EXAMPLE, policy_epoch=5, tag=7)):
        built = fields(inputs)
        env = {
            "network_id": hashlib.sha256(inputs["network_passphrase"].encode("utf-8")).digest(),
            "mandate": address_xdr(inputs["mandate"])[0],
            "recipient_ref": bytes.fromhex(inputs["recipient_ref"]),
            "country_policy_hash": bytes.fromhex(inputs["country_policy_hash"]),
            "policy_epoch": inputs["policy_epoch"],
            "valid_until": inputs["valid_until"],
            "destination": address_xdr(inputs["destination"])[0],
            "has_tag": 0 if inputs.get("tag") is None else 1,
            "tag": inputs.get("tag") or 0,
            "fields": built,
        }
        cases.append((env, {"fields": built, "message": message(built)}))
    return cases


def check_layouts(text, name, fenced_lines, problems):
    """Evaluates every layout block and address expression in the file against the
    bytes this script builds. Returns how many were evaluated. No fence is stripped
    or skipped here: every line in a fence has to be part of a layout."""
    definitions, current = [], None
    for number, line in fenced_lines:
        match = re.match(r"^(\w+)\s*=\s*(.*)$", line)
        if match:
            current = [match.group(1), number, match.group(2)]
            definitions.append(current)
        elif line.startswith((" ", "\t")) and line.strip() and current is not None:
            current[2] += " " + line.strip()
        elif line.strip():
            problems.append('%s:%d: cannot read "%s" in a layout block; every line there is part of a layout'
                            % (name, number, line.strip()))

    cases = layout_cases()
    known = {layout for _, built in cases for layout in built}
    seen = {}
    for layout, number, expression in definitions:
        if layout in seen:
            problems.append("%s:%d: the %s layout is written twice" % (name, number, layout))
            continue
        seen[layout] = number
        if layout not in known:
            problems.append('%s:%d: "%s" is not a layout this script builds' % (name, number, layout))
            continue
        for env, built in cases:
            if layout not in built:
                continue
            try:
                got = evaluate(expression, env)
            except LayoutError as err:
                problems.append("%s:%d: the %s layout: %s" % (name, number, layout, err))
                break
            if got != built[layout]:
                problems.append("%s:%d: the %s layout does not build the bytes this script builds: %s"
                                % (name, number, layout, " ".join(expression.split())))
                break
    for layout in sorted(known - set(seen)):
        problems.append("%s: no layout defines %s; it is missing or reworded, so it is no longer checked"
                        % (name, layout))

    # The address table's expressions, round tripped through the address encoder.
    _, account_key = decode_strkey(EXAMPLE["destination"])
    _, contract_id = decode_strkey(EXAMPLE["mandate"])
    rows = (
        ("Account `G`", {"ed25519_key": account_key}, address_xdr(EXAMPLE["destination"])[0]),
        ("Contract `C`", {"contract_id": contract_id}, address_xdr(EXAMPLE["mandate"])[0]),
    )
    for label, env, want in rows:
        match = re.search(r"^\|(?:.*\| )? ?%s \|.*?`(u32be[^`]*)`" % re.escape(label), text, re.M)
        if not match:
            problems.append("%s: no address table row for %s; it is missing or reworded, so it is no longer checked"
                            % (name, label))
            continue
        number = line_of(text, match.start())
        try:
            got = evaluate(match.group(1), env)
        except LayoutError as err:
            problems.append("%s:%d: the %s address: %s" % (name, number, label, err))
            continue
        if got != want:
            problems.append("%s:%d: the %s address does not encode as Address::to_xdr does: %s"
                            % (name, number, label, match.group(1)))
    return len(definitions) + len(rows)


def check_tables(text, name, compare, checked, problems):
    """Reads every table through its header. A row whose cells do not line up with
    the header is reported by name, because a row nobody reads checks nothing."""
    lines = text.split("\n")
    offsets, at = [], 0
    for line in lines:
        offsets.append(at)
        at += len(line) + 1
    seen = set()
    index = 0
    while index < len(lines) - 1:
        if not (lines[index].startswith("|") and re.fullmatch(r"\|(?: *:?-+:? *\|)+", lines[index + 1].strip())):
            index += 1
            continue
        header = [cell.strip() for cell in lines[index].strip().strip("|").split("|")]
        row = index + 2
        while row < len(lines) and lines[row].startswith("|"):
            line = lines[row]
            cells = line.strip().strip("|").split("|")
            if len(cells) != len(header):
                problems.append('%s:%d: the table row "%s" has %d cells under a header of %d, so it is not read'
                                % (name, row + 1, line.strip(), len(cells), len(header)))
            elif "Length" in header:
                label = next((c.strip() for c in cells if c.strip() in TABLE_LENGTHS), cells[0].strip())
                column = header.index("Length")
                cell = cells[column]
                if label not in TABLE_LENGTHS:
                    problems.append('%s:%d: the table row "%s" has no derivation; add it to TABLE_LENGTHS'
                                    % (name, row + 1, line.strip()))
                elif not re.fullmatch(r" *\d+ *", cell):
                    problems.append('%s:%d: the table row "%s" has no number in its Length column, so it is not read'
                                    % (name, row + 1, line.strip()))
                else:
                    seen.add(label)
                    start = line.index("|") + 1 + sum(len(c) + 1 for c in cells[:column]) + (len(cell) - len(cell.lstrip()))
                    value = cell.strip()
                    span = (offsets[row] + start, offsets[row] + start + len(value))
                    checked.add(span)
                    compare(TABLE_LENGTHS[label][0], span[0], int(value), TABLE_LENGTHS[label][1])
            row += 1
        index = row
    for label in sorted(set(TABLE_LENGTHS) - seen):
        if not any(("the table row" in p and label in p) for p in problems):
            problems.append("%s: no figure matched %s; it is missing or reworded, so it is no longer checked"
                            % (name, TABLE_LENGTHS[label][0]))


def load_exemptions(path, text, problems):
    """Returns [(entry name, set of spans)]. An entry says where a number sits and
    why it is not a length. It is never a bare value: the number stays a group of
    any digits, with words around it, so it cannot bless the same value elsewhere."""
    label = os.path.basename(path)
    try:
        with open(path, encoding="utf-8") as handle:
            entries = json.load(handle)
    except (OSError, ValueError) as err:
        raise InputError("cannot read %s: %s" % (path, err))
    if not isinstance(entries, list):
        raise InputError("%s must be a list of { name, pattern, reason }" % label)
    exempt = []
    for number, entry in enumerate(entries, start=1):
        entry = entry if isinstance(entry, dict) else {}
        entry_name = entry.get("name") or "entry %d" % number
        pattern, reason = entry.get("pattern"), entry.get("reason")
        if set(entry) != {"name", "pattern", "reason"} or not isinstance(entry.get("name"), str):
            problems.append('%s: "%s" must have exactly a name, a pattern and a reason' % (label, entry_name))
            continue
        if not isinstance(reason, str) or not reason.strip():
            problems.append('%s: "%s" has no reason' % (label, entry_name))
            continue
        try:
            compiled = re.compile(pattern, re.M)
        except (re.error, TypeError) as err:
            problems.append('%s: "%s" has a bad pattern: %s' % (label, entry_name, err))
            continue
        context = pattern.replace(r"(\d+)", "") if isinstance(pattern, str) else ""
        if compiled.groups != 1 or r"(\d+)" not in pattern or not re.search(r"[A-Za-z]{3}", context):
            problems.append('%s: "%s" is a bare value; its pattern must have one (\\d+) group and words around it'
                            % (label, entry_name))
            continue
        exempt.append((entry_name, {match.span(1) for match in compiled.finditer(text)}))
    return exempt


def sweep_digits(text, name, checked, exempt, fences, inline, problems):
    """Every digit run must be exempt by construction, derived or exempted by name.
    Anything else fails with its line and text, so the question is never whether the
    check recognises a form. Fences are exempt here alone: the bracket sweep and the
    layout evaluation above still read them. Returns the counts."""
    code = fences + inline
    counts = {"total": 0, "construction": 0, "derived": 0, "named": 0, "unexplained": 0}
    used = {entry: 0 for entry, _ in exempt}
    refused = []

    # Forms read as something else first, so each message says what it read.
    for match in HEX_LITERAL.finditer(text):
        if not inside(code, match.start()):
            digits = match.group(0).split()[0].split("-")[0]
            refused.append(match.span())
            problems.append('%s:%d: the figure "%s" reads as the hex literal %s, which is %d; write a length in decimal digits'
                            % (name, line_of(text, match.start()), match.group(0), digits, int(digits, 16)))
    for match in GROUPED_NUMBER.finditer(text):
        if not inside(code, match.start()):
            digits = match.group(0).split()[0].split("-")[0]
            refused.append(match.span())
            problems.append('%s:%d: the figure "%s" reads as %s with a thousands separator; write a length as plain digits'
                            % (name, line_of(text, match.start()), match.group(0), digits.replace(",", "")))
    for match in SPELLED_LENGTH.finditer(text):
        if not inside(code, match.start()):
            problems.append('%s:%d: the length "%s" is spelled in words; write it in digits so it can be derived'
                            % (name, line_of(text, match.start()), match.group(0)))

    hex_runs = [match.span() for match in HEX_RUN.finditer(text)]
    for match in re.finditer(r"\d+", text):
        start, end = match.span()
        counts["total"] += 1
        before = text[start - 1] if start else ""
        before2 = text[start - 2] if start > 1 else ""
        if inside(refused, start):
            counts["unexplained"] += 1
        elif inside(code, start) or inside(hex_runs, start):
            counts["construction"] += 1
        # An identifier: the run follows a letter (ed25519), or a letter and a
        # hyphen (SHA-256), or is an RFC number. A run followed by a letter is not
        # one, or 32bytes would pass.
        elif before.isalpha() or (before == "-" and before2.isalpha()) or text[max(0, start - 4):start] == "RFC ":
            counts["construction"] += 1
        elif (start, end) in checked:
            counts["derived"] += 1
        else:
            entry = next((entry for entry, spans in exempt if (start, end) in spans), None)
            if entry is not None:
                counts["named"] += 1
                used[entry] += 1
            else:
                counts["unexplained"] += 1
                problems.append('%s:%d: the number %s in "%s" is not covered by any derivation or named exemption'
                                % (name, line_of(text, start), match.group(0), line_text(text, start)))
    for entry, count in used.items():
        if count == 0:
            problems.append('%s: the exemption "%s" covers no number the sweep needs; remove it' % (name, entry))
    return counts


def check_doc(path, exemptions=EXEMPTIONS):
    """Pins the layout, compares the paired figures and sweeps every digit run.
    Returns an exit code."""
    try:
        text = open(path, encoding="utf-8").read()
    except OSError as err:
        raise InputError("cannot read %s: %s" % (path, err))
    derived = all_derivations()
    name = path.split("/")[-1]
    checked = set()
    problems = []
    found = 0

    def compare(label, offset, documented, key):
        nonlocal found
        found += 1
        if documented != derived[key]:
            problems.append("%s:%d: %s says %d, the layout derives %d (%s)"
                            % (name, line_of(text, offset), label, documented, derived[key], key))

    fences, inline, fenced_lines, closed = code_spans(text)
    if not closed:
        problems.append("%s: a code fence never closes" % name)

    layouts = check_layouts(text, name, fenced_lines, problems)

    for label, pattern, keys in DOC_CHECKS:
        matches = list(re.finditer(pattern, text, re.M))
        if not matches:
            problems.append("%s: no figure matched %s; it is missing or reworded, so it is no longer checked"
                            % (name, label))
            continue
        for match in matches:
            for index, key in enumerate(keys, start=1):
                checked.add(match.span(index))
                compare(label, match.start(index), int(match.group(index)), key)

    # Runs over the fenced text too, so the widths in the signed layout stay checked.
    for match in re.finditer(r"(\w+)\[(\d+)\]", text):
        field, width = match.group(1), int(match.group(2))
        key = BRACKET_WIDTHS.get(field)
        if key is None:
            problems.append("%s:%d: the width of %s[%d] is not covered by any derivation; add it to BRACKET_WIDTHS"
                            % (name, line_of(text, match.start(2)), field, width))
            continue
        checked.add(match.span(2))
        compare("the %s width" % field, match.start(2), width, key)

    check_tables(text, name, compare, checked, problems)

    exempt = load_exemptions(exemptions, text, problems)
    counts = sweep_digits(text, name, checked, exempt, fences, inline, problems)

    if problems:
        for problem in sorted(set(problems)):
            print("attestation-message: " + problem, file=sys.stderr)
        return 1
    print("%s: %d figure(s) checked, every one equals the layout" % (name, found))
    print("%s: %d layout expression(s) evaluated, every one builds the script's bytes" % (name, layouts))
    print("%s: %d digit run(s): %d exempt by construction, %d derived, %d exempt by name, %d unexplained"
          % (name, counts["total"], counts["construction"], counts["derived"], counts["named"], counts["unexplained"]))
    return 0


def check_constants(path):
    """Checks the attestation lengths in the constants file against the derivations."""
    try:
        data = json.load(open(path, encoding="utf-8"))
    except (OSError, ValueError) as err:
        raise InputError("cannot read %s: %s" % (path, err))
    constants = data.get("constants", data)
    derived = all_derivations()
    wanted = {
        "ATTESTATION_MESSAGE_BYTES": "message",
        "ATTESTER_KEY_BYTES": "attester_key",
        "ATTESTATION_SIGNATURE_BYTES": "signature",
    }
    name = path.split("/")[-1]
    problems = []
    for constant, key in wanted.items():
        if constant not in constants:
            problems.append("%s: %s is missing" % (name, constant))
        elif constants[constant] != derived[key]:
            problems.append("%s: %s is %s, the layout derives %d"
                            % (name, constant, constants[constant], derived[key]))
    if problems:
        for problem in problems:
            print("attestation-message: " + problem, file=sys.stderr)
        return 1
    print("%s: %d attestation length(s) checked, every one equals the layout" % (name, len(wanted)))
    return 0


def main(argv):
    if argv == ["--example"]:
        f = fields(EXAMPLE)
        m = message(f)
        print("fields  " + f.hex())
        print("sha256  " + hashlib.sha256(f).hexdigest())
        print("message " + m.hex())
        print("length  %d" % len(m))
        return 0
    if argv == ["--tenant-tag-example"]:
        ref = tenant_ref(TENANT_EXAMPLE["principal_uuid"], TENANT_EXAMPLE["project_uuid"])
        print("tenant_ref  " + ref.hex())
        print("mandate_xdr " + address_xdr(EXAMPLE["mandate"])[0].hex())
        print("tenant_tag  " + tenant_tag(ref, EXAMPLE["mandate"]).hex())
        return 0
    if len(argv) == 2 and argv[0] == "--xdr":
        xdr, tag = address_xdr(argv[1])
        print(xdr.hex() + ("" if tag is None else " tag %d" % tag))
        return 0
    if argv == ["--derive"]:
        rows = derivations()
        width = max(len(name) for name, _, _ in rows)
        for name, value, how in rows:
            print("%-*s %3d %-5s %s" % (width, name, value, "byte" if value == 1 else "bytes", how))
        for name, value, how in scalar_derivations():
            print("%-*s %3d %-5s %s" % (width, name, value, "", how))
        return 0
    if len(argv) == 2 and argv[0] == "--check-doc":
        return check_doc(argv[1])
    if len(argv) == 4 and argv[0] == "--check-doc" and argv[2] == "--exemptions":
        return check_doc(argv[1], argv[3])
    if len(argv) == 2 and argv[0] == "--check-constants":
        return check_constants(argv[1])
    print("usage: attestation-message.py --example | --tenant-tag-example | --xdr <strkey> | --derive"
          " | --check-doc <file> [--exemptions <file>] | --check-constants <file>", file=sys.stderr)
    return 2


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except InputError as err:
        print("attestation-message: %s" % err, file=sys.stderr)
        sys.exit(1)
