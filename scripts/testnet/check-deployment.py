#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Checks a deployment record such as deployments/testnet.json before it is
# written and again in CI. The record is committed, so it may hold public
# data only: it must have exactly the known fields, every account and
# contract must be a well formed public strkey with a valid checksum, every
# hash must be 64 lowercase hex digits, and no value anywhere may look like a
# secret seed. It must also name testnet and nothing else. A passkey wallet
# factory, once deployed, is recorded under the optional "wallet_factory".
#
# Standard library only.
#
# Usage: scripts/testnet/check-deployment.py FILE...
# Exit codes: 0 every file passes, 1 a file fails (reasons on stderr),
# 2 a file cannot be read or parsed.

import base64
import json
import re
import sys

TESTNET_PASSPHRASE = "Test SDF Network ; September 2015"

HEX64 = re.compile(r"^[0-9a-f]{64}$")
COMMIT = re.compile(r"^[0-9a-f]{40}$")
STAMP = re.compile(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$")
ASSET_CODE = re.compile(r"^[A-Za-z0-9]{1,12}$")
# Any strkey shaped run of characters that starts like a secret seed.
SECRET_LIKE = re.compile(r"S[A-Z2-7]{55}")

# Strkey version bytes: G (account) is 6 << 3, C (contract) is 2 << 3.
VERSION_ACCOUNT = 6 << 3
VERSION_CONTRACT = 2 << 3

TOP = {
    "network": str,
    "network_passphrase": str,
    "factory": str,
    "factory_wasm_hash": str,
    "mandate_wasm_hash": str,
    "spec_version": int,
    "supported_assets": list,
    "fee_address": str,
    "deployer": str,
    "commit": str,
    "soroban_tree": str,
    "deploy_transaction": str,
    "deployed_at": str,
}
OPTIONAL = {"wallet_factory": dict}
WALLET = {
    "factory": str,
    "factory_wasm_hash": str,
    "account_wasm_hash": str,
    "verifier": str,
    "verifier_wasm_hash": str,
    "policy": str,
    "policy_wasm_hash": str,
    "deny_list": list,
    "upstream_tag": str,
    "upstream_commit": str,
    "commit": str,
    "deploy_transaction": str,
    "deployed_at": str,
}
TAG = re.compile(r"^v[0-9]+\.[0-9]+\.[0-9]+$")
ASSET_REQUIRED = {"code", "issuer", "contract"}
ASSET_OPTIONAL = {"test_only"}


def crc16_xmodem(data):
    crc = 0
    for byte in data:
        crc ^= byte << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) if crc & 0x8000 else (crc << 1)
            crc &= 0xFFFF
    return crc


def strkey_ok(value, version):
    if not isinstance(value, str) or len(value) != 56:
        return False
    try:
        raw = base64.b32decode(value, casefold=False)
    except (ValueError, TypeError):
        return False
    if len(raw) != 35 or raw[0] != version:
        return False
    body, check = raw[:-2], raw[-2:]
    return crc16_xmodem(body) == int.from_bytes(check, "little")


def strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for k, v in value.items():
            yield k
            yield from strings(v)
    elif isinstance(value, list):
        for v in value:
            yield from strings(v)


def check(record):
    errors = []
    if not isinstance(record, dict):
        return ["the record is not a JSON object"]

    for text in strings(record):
        if SECRET_LIKE.search(text):
            errors.append("a value looks like a secret seed")
            break

    missing = sorted(set(TOP) - set(record))
    extra = sorted(set(record) - set(TOP) - set(OPTIONAL))
    if missing:
        errors.append(f"missing fields: {', '.join(missing)}")
    if extra:
        errors.append(f"unknown fields: {', '.join(extra)}")
    for key, kind in {**TOP, **OPTIONAL}.items():
        if key in record and (
            not isinstance(record[key], kind) or isinstance(record[key], bool)
        ):
            errors.append(f"{key} is not a {kind.__name__}")
    if errors:
        return errors

    if record["network"] != "testnet":
        errors.append("network is not testnet")
    if record["network_passphrase"] != TESTNET_PASSPHRASE:
        errors.append("network_passphrase is not the testnet passphrase")
    if not strkey_ok(record["factory"], VERSION_CONTRACT):
        errors.append("factory is not a contract id")
    for key in ("fee_address", "deployer"):
        if not strkey_ok(record[key], VERSION_ACCOUNT):
            errors.append(f"{key} is not an account public key")
    for key in ("factory_wasm_hash", "mandate_wasm_hash", "deploy_transaction"):
        if not HEX64.match(record[key]):
            errors.append(f"{key} is not 64 lowercase hex digits")
    for key in ("commit", "soroban_tree"):
        if not COMMIT.match(record[key]):
            errors.append(f"{key} is not a 40 digit git object id")
    if not STAMP.match(record["deployed_at"]):
        errors.append("deployed_at is not a UTC timestamp")
    if record["spec_version"] < 1:
        errors.append("spec_version is not positive")

    assets = record["supported_assets"]
    if not assets:
        errors.append("supported_assets is empty")
    contracts = []
    for i, asset in enumerate(assets):
        where = f"supported_assets[{i}]"
        if not isinstance(asset, dict):
            errors.append(f"{where} is not an object")
            continue
        keys = set(asset)
        if not ASSET_REQUIRED <= keys or not keys <= ASSET_REQUIRED | ASSET_OPTIONAL:
            errors.append(f"{where} must have code, issuer, contract and optionally test_only")
            continue
        if not isinstance(asset["code"], str) or not ASSET_CODE.match(asset["code"]):
            errors.append(f"{where}.code is not an asset code")
        if not strkey_ok(asset["issuer"], VERSION_ACCOUNT):
            errors.append(f"{where}.issuer is not an account public key")
        if not strkey_ok(asset["contract"], VERSION_CONTRACT):
            errors.append(f"{where}.contract is not a contract id")
        if "test_only" in asset and not isinstance(asset["test_only"], bool):
            errors.append(f"{where}.test_only is not a boolean")
        contracts.append(asset["contract"])
    if len(set(contracts)) != len(contracts):
        errors.append("supported_assets repeats a contract")
    if "wallet_factory" in record:
        errors.extend(check_wallet(record["wallet_factory"]))
    return errors


def check_wallet(wallet):
    errors = []
    missing = sorted(set(WALLET) - set(wallet))
    extra = sorted(set(wallet) - set(WALLET))
    if missing:
        errors.append(f"wallet_factory is missing fields: {', '.join(missing)}")
    if extra:
        errors.append(f"wallet_factory has unknown fields: {', '.join(extra)}")
    for key, kind in WALLET.items():
        if key in wallet and not isinstance(wallet[key], kind):
            errors.append(f"wallet_factory.{key} is not a {kind.__name__}")
    if errors:
        return errors
    for key in ("factory", "verifier", "policy"):
        if not strkey_ok(wallet[key], VERSION_CONTRACT):
            errors.append(f"wallet_factory.{key} is not a contract id")
    for key in ("factory_wasm_hash", "account_wasm_hash", "verifier_wasm_hash", "policy_wasm_hash", "deploy_transaction"):
        if not HEX64.match(wallet[key]):
            errors.append(f"wallet_factory.{key} is not 64 lowercase hex digits")
    for key in ("upstream_commit", "commit"):
        if not COMMIT.match(wallet[key]):
            errors.append(f"wallet_factory.{key} is not a 40 digit git object id")
    if not TAG.match(wallet["upstream_tag"]):
        errors.append("wallet_factory.upstream_tag is not a release tag")
    if not STAMP.match(wallet["deployed_at"]):
        errors.append("wallet_factory.deployed_at is not a UTC timestamp")
    deny = wallet["deny_list"]
    if not deny:
        errors.append("wallet_factory.deny_list is empty")
    for i, entry in enumerate(deny):
        if not (strkey_ok(entry, VERSION_ACCOUNT) or strkey_ok(entry, VERSION_CONTRACT)):
            errors.append(f"wallet_factory.deny_list[{i}] is not an account or contract id")
    if len(set(map(str, deny))) != len(deny):
        errors.append("wallet_factory.deny_list repeats an address")
    return errors


def main(paths):
    if not paths:
        print("usage: check-deployment.py FILE...", file=sys.stderr)
        return 2
    status = 0
    for path in paths:
        try:
            with open(path, encoding="utf-8") as f:
                record = json.load(f)
        except (OSError, ValueError) as e:
            print(f"check-deployment: {path}: {e}", file=sys.stderr)
            return 2
        errors = check(record)
        for error in errors:
            print(f"check-deployment: {path}: {error}", file=sys.stderr)
        if errors:
            status = 1
        else:
            print(f"check-deployment: {path}: ok")
    return status


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
