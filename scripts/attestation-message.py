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
#   scripts/attestation-message.py --check-doc spec/ENCODING.md
#       check every figure in the file against --derive. A figure that no
#       derivation covers fails too, so a new one cannot be added without a
#       derivation behind it
#   scripts/attestation-message.py --check-constants spec/constants.json
#       check the attestation lengths in the constants file against --derive
# Exit codes: 0 done, 1 an input is invalid or a figure disagrees, 2 bad usage.

import base64
import hashlib
import json
import re
import sys
import uuid

PREFIX = b"UJIRA/ATTEST/V2"
U64_MAX = (1 << 64) - 1

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


# Each figure in spec/ENCODING.md, as a pattern whose groups capture the
# documented numbers and the derivation each one must equal. A figure the file
# states and this table does not cover is reported, so the table cannot fall
# behind the file.
DOC_CHECKS = [
    ("the integer widths", r"integers of (\d+), (\d+), (\d+) and (\d+) bytes",
     ("u8", "u16be", "u32be", "u64be")),
    ("the ID length", r"`tenant_tag` are (\d+) bytes", ("sha256",)),
    ("the leg_key length", r"`leg_key` is (\d+) bytes", ("leg_key",)),
    ("the leg index width", r"the `payout_id` and (\d+) bytes? of leg index", ("u8",)),
    ("the address length prefix width", r"each after a (\d+)[- ]bytes? length prefix", ("u8",)),
    ("the UUID length", r"A UUID is its (\d+) bytes", ("uuid",)),
    ("the account address length in the table", r"^\| Account `G` \|.*\| (\d+) \|$", ("account_xdr",)),
    ("the contract address length in the table", r"^\| Contract `C` \|.*\| (\d+) \|$", ("contract_xdr",)),
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

# Every number the file states as a length must be covered by a check above.
GUARDS = [
    r"(\d+)[- ]bytes?\b",
    r"(\d+) ASCII bytes",
    r"(\d+)[- ]bits?\b",
    r"(\d+) zero bytes",
    r"(\d+) for an account",
    r"(\d+) for a contract",
    r"\w+\[(\d+)\]",
    r"^\|[^|]*\|[^|]*\| (\d+) \|$",
]


def line_of(text, offset):
    return text.count("\n", 0, offset) + 1


def check_doc(path):
    """Compares every figure in the file with the derivations. Returns an exit code."""
    try:
        text = open(path, encoding="utf-8").read()
    except OSError as err:
        raise InputError("cannot read %s: %s" % (path, err))
    derived = {name: value for name, value, _ in derivations()}
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

    for match in re.finditer(r"(\w+)\[(\d+)\]", text):
        field, width = match.group(1), int(match.group(2))
        key = BRACKET_WIDTHS.get(field)
        if key is None:
            problems.append("%s:%d: the width of %s[%d] is not covered by any derivation; add it to BRACKET_WIDTHS"
                            % (name, line_of(text, match.start(2)), field, width))
            continue
        checked.add(match.span(2))
        compare("the %s width" % field, match.start(2), width, key)

    # Case insensitive on purpose: the address table's header column is Bytes,
    # so a capitalised unit is the natural way for someone to write a length
    # here, and the guards have to see it.
    for guard in GUARDS:
        for match in re.finditer(guard, text, re.M | re.I):
            if match.span(1) not in checked:
                problems.append('%s:%d: the figure "%s" is not covered by any derivation; add a check for it'
                                % (name, line_of(text, match.start(1)), match.group(0).strip()))

    if problems:
        for problem in sorted(set(problems)):
            print("attestation-message: " + problem, file=sys.stderr)
        return 1
    print("%s: %d figure(s) checked, every one equals the layout" % (name, found))
    return 0


def check_constants(path):
    """Checks the attestation lengths in the constants file against the derivations."""
    try:
        data = json.load(open(path, encoding="utf-8"))
    except (OSError, ValueError) as err:
        raise InputError("cannot read %s: %s" % (path, err))
    constants = data.get("constants", data)
    derived = {name: value for name, value, _ in derivations()}
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
        return 0
    if len(argv) == 2 and argv[0] == "--check-doc":
        return check_doc(argv[1])
    if len(argv) == 2 and argv[0] == "--check-constants":
        return check_constants(argv[1])
    print("usage: attestation-message.py --example | --tenant-tag-example | --xdr <strkey> | --derive"
          " | --check-doc <file> | --check-constants <file>", file=sys.stderr)
    return 2


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except InputError as err:
        print("attestation-message: %s" % err, file=sys.stderr)
        sys.exit(1)
