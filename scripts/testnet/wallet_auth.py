#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Test passkeys for a passkey wallet on testnet, and the WebAuthn authorization
# such a wallet gives a call. The smoke test uses it to make a wallet and to let
# that wallet authorize the mandate factory's create.
#
# Passkey i of a name has the P-256 secret SHA-256("ujira-test-passkey:" ||
# name || ":" || i), as the vector runner derives them. That secret is public
# by construction, so a wallet made from these keys protects nothing; it is for
# testnet smoke tests only, never for funds.
#
# XDR is built by `stellar xdr encode` from its JSON form, so this script does
# no XDR encoding of its own.
#
# Usage:
#   wallet_auth.py signers NAME COUNT VERIFIER          the deploy's --signers
#   wallet_auth.py policies NAME COUNT THRESHOLD VERIFIER POLICY
#                                                        the deploy's --policies
#   wallet_auth.py sign NAME COUNT SIGNATURES VERIFIER WALLET EXPIRATION PASSPHRASE < tx.json
#       signs the WALLET's authorization entries of a decoded transaction with
#       its first SIGNATURES passkeys, valid until ledger EXPIRATION, and
#       prints the transaction JSON
#   wallet_auth.py contract-id DEPLOYER SALT_HEX PASSPHRASE
#       the address a contract deploys to from DEPLOYER with SALT_HEX
#   wallet_auth.py address-xdr ADDRESS                   an address as XDR hex
# Exit codes: 0 done, 1 bad input or a tool failed.

import base64
import hashlib
import json
import subprocess
import sys

from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.asymmetric.utils import Prehashed, decode_dss_signature

P256_ORDER = 0xFFFFFFFF00000000FFFFFFFFFFFFFFFFBCE6FAADA7179E84F3B9CAC2FC632551
RP_ID = "ujira.test"
# User present and user verified, which the verifier requires.
FLAGS = 0x05
# A new wallet has one rule, the constructor's Default rule.
DEFAULT_RULE = 0


def sha256(data):
    return hashlib.sha256(data).digest()


def passkey(name, index):
    seed = sha256(f"ujira-test-passkey:{name}:{index}".encode())
    return ec.derive_private_key(int.from_bytes(seed, "big"), ec.SECP256R1())


def public_hex(key):
    return key.public_key().public_bytes(
        serialization.Encoding.X962, serialization.PublicFormat.UncompressedPoint
    ).hex()


def keys(name, count):
    out = [passkey(name, i) for i in range(count)]
    return out


def encode(xdr_type, value):
    """XDR bytes of a JSON value, through the stellar CLI."""
    got = subprocess.run(
        ["stellar", "xdr", "encode", "--type", xdr_type, "--input", "json", "--output", "single-base64"],
        input=json.dumps(value), capture_output=True, text=True,
    )
    if got.returncode != 0:
        sys.exit(f"wallet_auth: cannot encode a {xdr_type}: {got.stderr.strip()}")
    return base64.b64decode(got.stdout.strip())


def signer_val(verifier, key_hex):
    return {"vec": [{"symbol": "External"}, {"address": verifier}, {"bytes": key_hex}]}


def symbol_map(entries):
    """An ScVal map from (symbol, value) pairs, sorted as the host requires."""
    return {"map": [{"key": {"symbol": k}, "val": v} for k, v in sorted(entries)]}


def base64url(data):
    return base64.urlsafe_b64encode(data).decode().rstrip("=")


def count_contexts(invocation):
    return 1 + sum(count_contexts(s) for s in invocation.get("sub_invocations", []))


def assertion(key, digest):
    """One WebAuthn assertion over the 32-byte challenge `digest`."""
    client = json.dumps(
        {"type": "webauthn.get", "challenge": base64url(digest), "origin": f"https://{RP_ID}", "crossOrigin": False},
        separators=(",", ":"),
    ).encode()
    auth_data = sha256(RP_ID.encode()) + bytes([FLAGS]) + bytes(4)
    to_sign = sha256(auth_data + sha256(client))
    r, s = decode_dss_signature(key.sign(to_sign, ec.ECDSA(Prehashed(hashes.SHA256()))))
    # The host verifies secp256r1 only in its low-s form.
    if s > P256_ORDER // 2:
        s = P256_ORDER - s
    signature = r.to_bytes(32, "big") + s.to_bytes(32, "big")
    data = symbol_map([
        ("authenticator_data", {"bytes": auth_data.hex()}),
        ("client_data", {"bytes": client.hex()}),
        ("signature", {"bytes": signature.hex()}),
    ])
    return encode("ScVal", data).hex()


def credential_entries(node, wallet):
    """Every authorization entry in a decoded transaction that the wallet signs."""
    if isinstance(node, dict):
        cred = node.get("credentials")
        if isinstance(cred, dict) and isinstance(cred.get("address"), dict) and cred["address"].get("address") == wallet:
            yield node
        for v in node.values():
            yield from credential_entries(v, wallet)
    elif isinstance(node, list):
        for v in node:
            yield from credential_entries(v, wallet)


def sign(name, count, signatures, verifier, wallet, expiration, passphrase, tx):
    network_id = sha256(passphrase.encode()).hex()
    found = 0
    for entry in credential_entries(tx, wallet):
        found += 1
        cred = entry["credentials"]["address"]
        cred["signature_expiration_ledger"] = expiration
        preimage = {"soroban_authorization": {
            "network_id": network_id,
            "nonce": cred["nonce"],
            "signature_expiration_ledger": expiration,
            "invocation": entry["root_invocation"],
        }}
        payload = sha256(encode("HashIdPreimage", preimage))
        rules = {"vec": [{"u32": DEFAULT_RULE}] * count_contexts(entry["root_invocation"])}
        digest = sha256(encode("ScVal", symbol_map([
            ("account", {"address": wallet}),
            ("context_rule_ids", rules),
            ("signature_payload", {"bytes": payload.hex()}),
        ])))
        signers = []
        for key in keys(name, count)[:signatures]:
            signers.append((public_hex(key), assertion(key, digest)))
        signers.sort()
        cred["signature"] = symbol_map([
            ("context_rule_ids", rules),
            ("signers", {"map": [{"key": signer_val(verifier, k), "val": {"bytes": s}} for k, s in signers]}),
        ])
    if found == 0:
        sys.exit(f"wallet_auth: the transaction has no authorization entry for {wallet}")
    return tx


def strkey_contract(raw):
    body = bytes([2 << 3]) + raw
    crc = 0
    for byte in body:
        crc ^= byte << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) if crc & 0x8000 else (crc << 1)
            crc &= 0xFFFF
    return base64.b32encode(body + crc.to_bytes(2, "little")).decode()


def main(argv):
    if not argv:
        sys.exit("usage: wallet_auth.py signers|policies|sign|contract-id|address-xdr ...")
    cmd, args = argv[0], argv[1:]
    if cmd == "signers" and len(args) == 3:
        name, count, verifier = args[0], int(args[1]), args[2]
        print(json.dumps([{"External": [verifier, public_hex(k)]} for k in keys(name, count)]))
    elif cmd == "policies" and len(args) == 5:
        name, count, threshold, verifier, policy = args[0], int(args[1]), int(args[2]), args[3], args[4]
        weights = sorted(public_hex(k) for k in keys(name, count))
        params = symbol_map([
            ("signer_weights", {"map": [{"key": signer_val(verifier, k), "val": {"u32": 1}} for k in weights]}),
            ("threshold", {"u32": threshold}),
        ])
        print(json.dumps({policy: params}))
    elif cmd == "sign" and len(args) == 7:
        name, count, signatures, verifier, wallet = args[0], int(args[1]), int(args[2]), args[3], args[4]
        expiration, passphrase = int(args[5]), args[6]
        tx = json.load(sys.stdin)
        print(json.dumps(sign(name, count, signatures, verifier, wallet, expiration, passphrase, tx)))
    elif cmd == "contract-id" and len(args) == 3:
        deployer, salt_hex, passphrase = args
        preimage = {"contract_id": {
            "network_id": sha256(passphrase.encode()).hex(),
            "contract_id_preimage": {"address": {"address": deployer, "salt": salt_hex}},
        }}
        print(strkey_contract(sha256(encode("HashIdPreimage", preimage))))
    elif cmd == "address-xdr" and len(args) == 1:
        print(encode("ScVal", {"address": args[0]}).hex())
    else:
        sys.exit("wallet_auth: bad usage")


if __name__ == "__main__":
    main(sys.argv[1:])
