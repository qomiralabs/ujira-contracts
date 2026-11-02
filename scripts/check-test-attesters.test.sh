#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Self test of scripts/check-test-attesters.py. Builds throwaway repositories
# holding a copy of the fixture, plants a test key in one form and one place at
# a time and checks the verdict, then runs the check on this repository.
#
# Usage: scripts/check-test-attesters.test.sh
# Exit codes: 0 every case behaved, 1 a case did not.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
check="$here/check-test-attesters.py"
fixture=spikes/attestation-parity/fixture.json
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fails=0
passes=0

# The keys come from the fixture and the encodings are computed here, so this
# file holds no key shaped literal of its own.
form() {
  python3 -I - "$root/$fixture" "$1" "$2" <<'PY'
import base64, json, sys
path, name, kind = sys.argv[1:]
key = json.load(open(path))["keys"][name]["public_key"].removeprefix("0x")
raw = bytes.fromhex(key)
def crc16(data):
    crc = 0
    for byte in data:
        crc ^= byte << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) if crc & 0x8000 else (crc << 1)
            crc &= 0xFFFF
    return crc
body = bytes([6 << 3]) + raw
print({
    "hex": key,
    "0xhex": "0x" + key,
    "upper": key.upper(),
    "strkey": base64.b32encode(body + crc16(body).to_bytes(2, "little")).decode(),
    "base64": base64.b64encode(raw).decode(),
}[kind])
PY
}

# repo <name>: a repository with only the real fixture in it.
repo() {
  mkdir -p "$work/$1/$(dirname "$fixture")"
  cp "$root/$fixture" "$work/$1/$fixture"
  git -C "$work/$1" init -q
  git -C "$work/$1" add -A
}

# plant <repo> <path> <content>
plant() {
  mkdir -p "$work/$1/$(dirname "$2")"
  printf '%s\n' "$3" >"$work/$1/$2"
  git -C "$work/$1" add -A
}

# expect <name> <want exit code> <repo> [text the output must hold]
expect() {
  local name=$1 want=$2 dir=$3 text=${4:-} got=0 output
  output=$("$check" "$dir" 2>&1) || got=$?
  if [ "$got" != "$want" ]; then
    echo "FAIL $name: exit $got, want $want: $output"
    fails=$((fails + 1))
  elif [ -n "$text" ] && ! printf '%s' "$output" | grep -qF -- "$text"; then
    echo "FAIL $name: output lacks '$text': $output"
    fails=$((fails + 1))
  else
    echo "ok   $name"
    passes=$((passes + 1))
  fi
}

repo clean
plant clean config/attester.json '{ "attester": "not a key" }'
expect "a clean tree passes" 0 "$work/clean" "no test attester key"

# A config file naming the key fails, for each key and each way of writing it.
for name in attester other_signer; do
  for kind in hex 0xhex upper strkey base64; do
    repo "$name-$kind"
    plant "$name-$kind" config/attester.json "{ \"attester\": \"$(form "$name" "$kind")\" }"
    expect "a config naming $name as $kind fails" 1 "$work/$name-$kind" "config/attester.json: the public test key $name"
  done
done

repo raw
python3 -I - "$root/$fixture" "$work/raw/attester.bin" <<'PY'
import json, sys
key = json.load(open(sys.argv[1]))["keys"]["attester"]["public_key"].removeprefix("0x")
open(sys.argv[2], "wb").write(b"\0prefix" + bytes.fromhex(key) + b"suffix\0")
PY
git -C "$work/raw" add -A
expect "the raw bytes in a binary file fail" 1 "$work/raw" "as raw bytes"

key=$(form attester 0xhex)
for path in deployments/testnet.json scripts/testnet/deploy.sh scripts/e2e/run.sh; do
  case_name=$(printf '%s' "$path" | tr '/.' '--')
  repo "$case_name"
  plant "$case_name" "$path" "attester=$key"
  expect "the deployment path $path fails" 1 "$work/$case_name" "$path"
done

repo untracked
mkdir -p "$work/untracked/config"
printf '%s\n' "$key" >"$work/untracked/config/new.env.json"
expect "a new file git does not track yet fails" 1 "$work/untracked" "config/new.env.json"

repo tests
plant tests soroban/vector-runner/tests/a.rs "$key"
plant tests exploits/src/a.rs "$key"
plant tests spec/ENCODING.md "$key"
expect "test code and the worked example may name a key" 0 "$work/tests"

# A path that only starts like a test tree is not test code, and neither is
# the source of a contract.
repo prefix
plant prefix tests-config/attester.json "$key"
expect "a directory that only starts like a test tree fails" 1 "$work/prefix" "tests-config/attester.json"
repo mandate-src
plant mandate-src soroban/contracts/mandate/src/config.rs "$key"
expect "contract source fails" 1 "$work/mandate-src" "soroban/contracts/mandate/src/config.rs"

repo changed
python3 -I - "$work/changed/$fixture" <<'PY'
import json, sys
p = sys.argv[1]
d = json.load(open(p))
d["keys"]["attester"]["public_key"] = "0x" + "ab" * 32
json.dump(d, open(p, "w"))
PY
git -C "$work/changed" add -A
expect "a fixture with a different key fails until the check is updated" 1 "$work/changed" "update KEYS"

repo no-fixture
git -C "$work/no-fixture" rm -qf "$fixture"
expect "a missing fixture fails" 1 "$work/no-fixture" "cannot read its keys"

mkdir -p "$work/not-a-repo"
expect "a folder outside git cannot run" 2 "$work/not-a-repo" "cannot list files"

expect "this repository passes" 0 "$root" "no test attester key"

echo "$passes passed, $fails failed"
[ "$fails" = 0 ]
