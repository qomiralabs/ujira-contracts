#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Self test of scripts/testnet/check-deployment.py. Builds a valid record
# from strkeys generated here, so no key shaped literal is committed, checks
# it passes, then breaks it one way at a time and checks each break fails.
# Last, the committed deployments/*.json must pass.
#
# Usage: scripts/testnet/check-deployment.test.sh
# Exit codes: 0 every case behaved, 1 a case did not.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
check="$here/check-deployment.py"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# Writes the valid record, then one variant per case, each as CASE.json.
python3 - "$tmp" <<'PY'
import base64, copy, json, os, sys

out = sys.argv[1]

def crc16(data):
    crc = 0
    for byte in data:
        crc ^= byte << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) if crc & 0x8000 else (crc << 1)
            crc &= 0xFFFF
    return crc

def strkey(version, payload=None):
    body = bytes([version]) + (payload or os.urandom(32))
    return base64.b32encode(body + crc16(body).to_bytes(2, "little")).decode()

account = lambda: strkey(6 << 3)
contract = lambda: strkey(2 << 3)
seed = lambda: strkey(18 << 3)

good = {
    "network": "testnet",
    "network_passphrase": "Test SDF Network ; September 2015",
    "factory": contract(),
    "factory_wasm_hash": "ab" * 32,
    "mandate_wasm_hash": "cd" * 32,
    "spec_version": 1,
    "supported_assets": [
        {"code": "USDC", "issuer": account(), "contract": contract()},
        {"code": "UJTEST", "issuer": account(), "contract": contract(), "test_only": True},
    ],
    "fee_address": account(),
    "deployer": account(),
    "commit": "1" * 40,
    "soroban_tree": "2" * 40,
    "deploy_transaction": "ef" * 32,
    "deployed_at": "2026-10-31T12:00:00Z",
}

def write(name, record):
    with open(os.path.join(out, name + ".json"), "w") as f:
        json.dump(record, f)

write("good", good)

def variant(name, change):
    record = copy.deepcopy(good)
    change(record)
    write("bad-" + name, record)

def broken_checksum(key):
    return key[:-1] + ("A" if key[-1] != "A" else "B")

variant("secret-field", lambda r: r.update(deployer_secret=seed()))
variant("secret-in-value", lambda r: r.update(deployed_at=seed()))
variant("secret-as-deployer", lambda r: r.update(deployer=seed()))
variant("secret-in-asset", lambda r: r["supported_assets"][0].update(issuer=seed()))
variant("mainnet", lambda r: r.update(network="mainnet"))
variant("mainnet-passphrase", lambda r: r.update(network_passphrase="Public Global Stellar Network ; September 2015"))
variant("missing-factory", lambda r: r.pop("factory"))
variant("unknown-field", lambda r: r.update(note="hello"))
variant("factory-is-account", lambda r: r.update(factory=account()))
variant("fee-is-contract", lambda r: r.update(fee_address=contract()))
variant("bad-checksum", lambda r: r.update(deployer=broken_checksum(r["deployer"])))
variant("hash-uppercase", lambda r: r.update(mandate_wasm_hash="AB" * 32))
variant("hash-short", lambda r: r.update(factory_wasm_hash="ab" * 31))
variant("commit-short", lambda r: r.update(commit="1" * 39))
variant("spec-version-string", lambda r: r.update(spec_version="1"))
variant("spec-version-bool", lambda r: r.update(spec_version=True))
variant("no-assets", lambda r: r.update(supported_assets=[]))
variant("asset-repeated", lambda r: r["supported_assets"][1].update(contract=r["supported_assets"][0]["contract"]))
variant("asset-extra-key", lambda r: r["supported_assets"][0].update(secret="x"))
variant("asset-test-only-string", lambda r: r["supported_assets"][1].update(test_only="yes"))
variant("stamp-local", lambda r: r.update(deployed_at="2026-10-31 12:00:00"))
variant("not-object", lambda r: (r.clear()))
PY
printf '[1, 2]\n' >"$tmp/bad-array.json"
printf '{not json\n' >"$tmp/unreadable.json"

fail=0
run() {
  local want=$1 file=$2 got=0
  python3 "$check" "$file" >"$tmp/out" 2>&1 || got=$?
  if [ "$got" -ne "$want" ]; then
    echo "FAIL $(basename "$file"): exit $got, want $want" >&2
    cat "$tmp/out" >&2
    fail=1
  else
    echo "ok   $(basename "$file") (exit $got)"
  fi
}

run 0 "$tmp/good.json"
for f in "$tmp"/bad-*.json; do
  run 1 "$f"
done
run 2 "$tmp/unreadable.json"

# The committed records must pass as they are.
shopt -s nullglob
for f in "$root"/deployments/*.json; do
  run 0 "$f"
done

if [ "$fail" -ne 0 ]; then
  echo "check-deployment.test: failed" >&2
  exit 1
fi
echo "check-deployment.test: all cases passed"
