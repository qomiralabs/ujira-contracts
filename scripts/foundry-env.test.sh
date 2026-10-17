#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Self test for foundry-env.sh. Each case runs the wrapper against a throwaway
# foundry.toml and checks the exit code and output. The last cases use the
# real celo/foundry.toml and the real forge, when forge is installed, so a
# Foundry upgrade that stops honoring the profile shows up here.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
script="$here/foundry-env.sh"
work=$(cd "$(mktemp -d)" && pwd -P)
trap 'rm -rf "$work"' EXIT

passed=0
failures=0

# expect <case name> <exit code> <command>... ; output checks come from $want_out
# and $reject_out, set by the caller for the next case only.
want_out=""
reject_out=""
expect() {
  local name=$1 want=$2
  shift 2
  local out code=0
  out=$("$@" 2>&1) || code=$?
  if [ "$code" -ne "$want" ]; then
    echo "FAIL: $name: exit $code, want $want"
    printf '%s\n' "$out" | sed 's/^/    /'
    failures=$((failures + 1))
  elif [ -n "$want_out" ] && ! printf '%s\n' "$out" | grep -qF -- "$want_out"; then
    echo "FAIL: $name: output does not mention '$want_out'"
    printf '%s\n' "$out" | sed 's/^/    /'
    failures=$((failures + 1))
  elif [ -n "$reject_out" ] && printf '%s\n' "$out" | grep -qF -- "$reject_out"; then
    echo "FAIL: $name: output mentions '$reject_out'"
    printf '%s\n' "$out" | sed 's/^/    /'
    failures=$((failures + 1))
  else
    echo "ok: $name"
    passed=$((passed + 1))
  fi
  want_out=""
  reject_out=""
}

config="$work/foundry.toml"
cat >"$config" <<'TOML'
[profile.default]
src = "src"

[profile.ci] # heavier fuzzing
fuzz = { runs = 10000 }

[profile.default.fuzz]
runs = 1000
TOML

show_env='env | grep -E "^(FOUNDRY|DAPP)_" | sort || true'

want_out="FOUNDRY_PROFILE=default"
expect "default profile is exported" 0 \
  "$script" --config "$config" -- bash -c "$show_env"

want_out="FOUNDRY_PROFILE=ci"
expect "a defined profile with a trailing comment is accepted" 0 \
  "$script" --profile ci --config "$config" -- bash -c "$show_env"

reject_out="FOUNDRY_DENY"
expect "FOUNDRY_ variables are cleared" 0 \
  env FOUNDRY_DENY=never FOUNDRY_SOLC=0.8.0 FOUNDRY_SRC=elsewhere \
  "$script" --config "$config" -- bash -c "$show_env"

reject_out="DAPP_SOLC_VERSION"
expect "legacy DAPP_ variables are cleared" 0 \
  env DAPP_SOLC_VERSION=0.8.0 "$script" --config "$config" -- bash -c "$show_env"

want_out="FOUNDRY_PROFILE=default"
expect "an inherited FOUNDRY_PROFILE is replaced" 0 \
  env FOUNDRY_PROFILE=ci "$script" --config "$config" -- bash -c "$show_env"

want_out="cleared 3"
expect "the cleared count is reported, values are not" 0 \
  env FOUNDRY_A=secret-a FOUNDRY_B=secret-b DAPP_C=secret-c \
  "$script" --config "$config" -- true

reject_out="secret-a"
expect "cleared values never reach the output" 0 \
  env FOUNDRY_A=secret-a "$script" --config "$config" -- true

want_out="profile 'nope' is not defined"
expect "an unknown profile exits 2" 2 \
  "$script" --profile nope --config "$config" -- true

want_out="not a valid profile name"
expect "a nested table name is not a profile" 2 \
  "$script" --profile default.fuzz --config "$config" -- true

want_out="not a valid profile name"
expect "a pattern in the profile name exits 2" 2 \
  "$script" --profile '.*' --config "$config" -- true

want_out="no foundry.toml"
expect "a missing config exits 2" 2 \
  "$script" --config "$work/missing.toml" -- true

want_out="no command given"
expect "no command exits 2" 2 "$script" --config "$config" --

want_out="put the command after --"
expect "a command without -- exits 2" 2 "$script" --config "$config" true

expect "the command's exit code is passed through" 7 \
  "$script" --config "$config" -- bash -c 'exit 7'

# End to end with the real config and forge.
if command -v forge >/dev/null 2>&1; then
  real="$here/../celo/foundry.toml"

  want_out='"solc": "0.8.37"'
  expect "forge sees foundry.toml, not an inherited FOUNDRY_SOLC" 0 \
    env FOUNDRY_SOLC=0.8.20 "$script" --config "$real" -- \
    bash -c "cd '$here/../celo' && forge config --json | jq '{solc}'"

  want_out='"deny": "warnings"'
  expect "forge sees foundry.toml, not an inherited FOUNDRY_DENY" 0 \
    env FOUNDRY_DENY=never "$script" --config "$real" -- \
    bash -c "cd '$here/../celo' && forge config --json | jq '{deny}'"

  want_out="profile 'nope' is not defined"
  expect "an unknown profile is refused for the real config" 2 \
    "$script" --profile nope --config "$real" -- forge --version
else
  echo "skip: forge is not installed, end to end cases not run"
fi

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
