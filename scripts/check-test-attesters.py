#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Refuses the public test attester keys anywhere outside test code.
#
# The attestation parity fixture signs with two ed25519 keys whose seeds are
# the SHA-256 of labels written in the fixture, so anyone can rebuild the
# private keys and sign as them. A mandate or service configured with one of
# them as its attester would accept attestations from anybody. The contract
# cannot tell, because it only checks the key's length.
#
# Every file git knows about is read, tracked or new, except .gitignored ones.
# A key is found as hex in either case, as a G strkey, as base64 or as raw
# bytes. Files are refused by default: only the test trees and the worked
# example files below may name a key, and the deployment paths never may.
# The two keys are also compared with the fixture's, so changing its labels
# cannot leave this check guarding keys nobody uses any more.
#
# Standard library only.
#
# Usage: scripts/check-test-attesters.py [repo root, default this repo]
# Exit codes: 0 clean, 1 a test key was found or the fixture disagrees, 2 the
# check could not run.

import base64
import json
import pathlib
import subprocess
import sys

KEYS = {
    "attester": "421adb50c10fe73fb1fc0d4211a16911a188bb001eaf0391b479316026141d48",
    "other_signer": "a51a56cfabe1fbb71d7172c52fd2bb9d715f75fb50e65b060961fb1690a59cab",
}

FIXTURE = "spikes/attestation-parity/fixture.json"

# Test code, which is where these keys belong.
TEST_TREES = (
    "exploits/",
    "scripts/fixtures/",
    "soroban/spikes/",
    "soroban/vector-runner/",
    "spec/vectors/",
    "spikes/attestation-parity/",
    "tests/",
)

# Single files that show a key without configuring anything: the worked
# attestation example, the tool that recomputes it, the spike note and the
# mandate's own message test.
EXAMPLES = (
    "scripts/attestation-message.py",
    "soroban/contracts/mandate/src/test/attestation.rs",
    "spec/ENCODING.md",
    "spikes/attestation-parity.md",
)

# Checked first, so widening a list above can never excuse one of these.
NEVER = ("deployments/", "scripts/e2e/", "scripts/testnet/")

# This check and its self test only compute the encodings or name the keys to
# look for.
SELF = ("scripts/check-test-attesters.py", "scripts/check-test-attesters.test.sh")


def crc16(data):
    crc = 0
    for byte in data:
        crc ^= byte << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) if crc & 0x8000 else (crc << 1)
            crc &= 0xFFFF
    return crc


def strkey(raw):
    body = bytes([6 << 3]) + raw
    return base64.b32encode(body + crc16(body).to_bytes(2, "little"))


def forms(key_hex):
    raw = bytes.fromhex(key_hex)
    return {
        "hex": key_hex.encode(),
        "strkey": strkey(raw),
        "base64": base64.b64encode(raw).rstrip(b"="),
        "raw bytes": raw,
    }


def allowed(path):
    if path.startswith(NEVER):
        return False
    return path.startswith(TEST_TREES) or path in EXAMPLES or path in SELF


def listed_files(root):
    out = subprocess.run(
        ["git", "-C", str(root), "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        check=True,
        capture_output=True,
    ).stdout
    return sorted({p.decode() for p in out.split(b"\0") if p})


def fixture_disagrees(root):
    try:
        keys = json.loads((root / FIXTURE).read_text(encoding="utf-8"))["keys"]
        found = {name: keys[name]["public_key"].removeprefix("0x") for name in KEYS}
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as e:
        return [f"{FIXTURE}: cannot read its keys ({e!r})"]
    return [
        f"{FIXTURE}: {name} is {found[name]}, this check guards {KEYS[name]}; update KEYS"
        for name in KEYS
        if found[name] != KEYS[name]
    ]


def scan(root, path, patterns):
    target = root / path
    # A deleted but still tracked file, a directory or a dangling symlink has
    # no bytes of its own to hold a key.
    if not target.is_file():
        return []
    data = target.read_bytes()
    folded = data.lower()
    hits = []
    for name, form, pattern in patterns:
        haystack = folded if form == "hex" else data
        if pattern in haystack:
            hits.append(f"{path}: the public test key {name} as {form}")
    return hits


def main():
    root = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else pathlib.Path(__file__).parent.parent)
    try:
        files = listed_files(root)
    except (OSError, subprocess.CalledProcessError) as e:
        print(f"check-test-attesters: cannot list files in {root}: {e}", file=sys.stderr)
        return 2

    patterns = [(n, f, p) for n, k in KEYS.items() for f, p in forms(k).items()]
    problems = fixture_disagrees(root)
    checked = 0
    for path in files:
        if allowed(path):
            continue
        checked += 1
        try:
            problems.extend(scan(root, path, patterns))
        except OSError as e:
            print(f"check-test-attesters: cannot read {path}: {e}", file=sys.stderr)
            return 2

    if problems:
        for line in problems:
            print(line, file=sys.stderr)
        print(
            "check-test-attesters: these keys are public test keys. Anyone can sign "
            "as them, so they must never be configured as an attester.",
            file=sys.stderr,
        )
        return 1
    print(f"check-test-attesters: {checked} non-test file(s) checked, no test attester key")
    return 0


if __name__ == "__main__":
    sys.exit(main())
