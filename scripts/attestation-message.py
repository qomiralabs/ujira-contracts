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
#   scripts/attestation-message.py --xdr <strkey>
#       print the Address::to_xdr bytes of a G, C or M address (an M address
#       gives its base G address and its id)
# Exit codes: 0 done, 1 an input is invalid, 2 bad usage.

import base64
import hashlib
import sys

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


def message(field_bytes):
    return PREFIX + hashlib.sha256(field_bytes).digest()


def main(argv):
    if argv == ["--example"]:
        f = fields(EXAMPLE)
        m = message(f)
        print("fields  " + f.hex())
        print("sha256  " + hashlib.sha256(f).hexdigest())
        print("message " + m.hex())
        print("length  %d" % len(m))
        return 0
    if len(argv) == 2 and argv[0] == "--xdr":
        xdr, tag = address_xdr(argv[1])
        print(xdr.hex() + ("" if tag is None else " tag %d" % tag))
        return 0
    print("usage: attestation-message.py --example | --xdr <strkey>", file=sys.stderr)
    return 2


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except InputError as err:
        print("attestation-message: %s" % err, file=sys.stderr)
        sys.exit(1)
