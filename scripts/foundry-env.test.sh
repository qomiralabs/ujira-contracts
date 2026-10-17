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

# The unit cases run from an empty directory with an empty HOME, so a .env or
# a user-level foundry.toml on the machine running the test cannot leak in.
real_home=$HOME
export HOME="$work/home"
mkdir -p "$HOME" "$work/clean"
cd "$work/clean"

# in_dir <dir> <command>... runs the command from <dir>. expect runs it in a
# subshell, so the cd does not leak into later cases.
in_dir() {
  cd "$1"
  shift
  "$@"
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

# Case insensitive, because forge reads these names in any case.
show_env='env | grep -iE "^(FOUNDRY|DAPP)_" | sort || true'

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

reject_out="foundry_deny"
expect "lowercase foundry_ variables are cleared" 0 \
  env foundry_deny=never foundry_optimizer_runs=1 \
  "$script" --config "$config" -- bash -c "$show_env"

reject_out="Foundry_Deny"
expect "mixed case Foundry_ variables are cleared" 0 \
  env Foundry_Deny=never FoUnDrY_Solc=0.8.0 \
  "$script" --config "$config" -- bash -c "$show_env"

reject_out="dapp_solc_version"
expect "lowercase dapp_ variables are cleared" 0 \
  env dapp_solc_version=0.8.0 "$script" --config "$config" -- bash -c "$show_env"

reject_out="Dapp_Solc_Version"
expect "mixed case Dapp_ variables are cleared" 0 \
  env Dapp_Solc_Version=0.8.0 "$script" --config "$config" -- bash -c "$show_env"

want_out="FOUNDRY_PROFILE=default"
reject_out="foundry_profile"
expect "a lowercase foundry_profile cannot pick the profile" 0 \
  env foundry_profile=ci "$script" --config "$config" -- bash -c "$show_env"

want_out="cleared 4"
expect "every case variant is counted" 0 \
  env FOUNDRY_A=1 foundry_b=1 Foundry_C=1 dApP_d=1 \
  "$script" --config "$config" -- true

want_out="both kept"
# shellcheck disable=SC2016 # expanded by the inner shell, on purpose
expect "names that only start like FOUNDRY are kept" 0 \
  env FOUNDRYX_KEEP=1 FOUNDRY=1 "$script" --config "$config" -- \
  bash -c '[ "${FOUNDRYX_KEEP:-}" = 1 ] && [ "${FOUNDRY:-}" = 1 ] && echo "both kept"'

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

# forge loads .env on its own, after this wrapper has cleaned the environment,
# walking up from its working directory and from the project root.
mkdir -p "$work/envhere"
printf 'FOUNDRY_DENY=never\n' >"$work/envhere/.env"
want_out="refusing to run: $work/envhere/.env exists"
expect "a .env in the working directory is refused" 2 \
  in_dir "$work/envhere" "$script" --config "$config" -- true

mkdir -p "$work/envup/a/b"
printf 'FOUNDRY_NO_MATCH_TEST=.*\n' >"$work/envup/.env"
want_out="refusing to run: $work/envup/.env exists"
expect "a .env in a parent of the working directory is refused" 2 \
  in_dir "$work/envup/a/b" "$script" --config "$config" -- true

mkdir -p "$work/envcfg"
cp "$config" "$work/envcfg/foundry.toml"
printf 'foundry_no_match_test=.*\n' >"$work/envcfg/.env"
want_out="refusing to run: $work/envcfg/.env exists"
expect "a .env next to the config is refused" 2 \
  "$script" --config "$work/envcfg/foundry.toml" -- true

mkdir -p "$work/envlink"
ln -s "$work/nowhere" "$work/envlink/.env"
want_out="refusing to run: $work/envlink/.env exists"
expect "a dangling .env symlink is refused" 2 \
  in_dir "$work/envlink" "$script" --config "$config" -- true

mkdir -p "$work/userhome/.foundry"
printf 'no_match_test = ".*"\n' >"$work/userhome/.foundry/foundry.toml"
want_out="refusing to run: $work/userhome/.foundry/foundry.toml exists"
expect "a user-level foundry.toml is refused" 2 \
  env HOME="$work/userhome" "$script" --config "$config" -- true

want_out="HOME is not set"
expect "an unset HOME is refused" 2 \
  env -u HOME "$script" --config "$config" -- true

want_out="refusing to run: FOUNDRY_CONFIG is set"
expect "FOUNDRY_CONFIG is refused, not just cleared" 2 \
  env FOUNDRY_CONFIG="$work/other.toml" "$script" --config "$config" -- true

want_out="refusing to run: foundry_config is set"
expect "a lowercase foundry_config is refused" 2 \
  env foundry_config="$work/other.toml" "$script" --config "$config" -- true

want_out="refusing to run: inherited variable 'FOUNDRY_A-B' is not a valid shell name"
expect "an inherited FOUNDRY_ name that is not an identifier is refused" 2 \
  env 'FOUNDRY_A-B=1' "$script" --config "$config" -- true

want_out="refusing to run: inherited variable 'dapp_x.y' is not a valid shell name"
expect "an inherited dapp_ name that is not an identifier is refused" 2 \
  env 'dapp_x.y=1' "$script" --config "$config" -- true

expect "an unrelated name that is not an identifier is left alone" 0 \
  env 'OTHER-NAME=1' "$script" --config "$config" -- true

mkdir -p "$work/othercfg"
printf '[profile.default]\nsrc = "elsewhere"\n' >"$work/othercfg/foundry.toml"
want_out="refusing to run: $work/othercfg/foundry.toml is not $config"
expect "a different foundry.toml in the working directory is refused" 2 \
  in_dir "$work/othercfg" "$script" --config "$config" -- true

expect "the config's own directory is accepted" 0 \
  in_dir "$work" "$script" --config "$config" -- true

# End to end with the real config and forge, which needs the real HOME for
# its compiler cache.
export HOME="$real_home"
cd "$here/.."
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

  want_out='"deny":"warnings","optimizer_runs":200'
  expect "forge sees foundry.toml, not lowercase or mixed case overrides" 0 \
    env foundry_deny=never Foundry_Optimizer_Runs=1 "$script" --config "$real" -- \
    bash -c "cd '$here/../celo' && forge config --json | jq -c '{deny,optimizer_runs}'"

  want_out="profile 'nope' is not defined"
  expect "an unknown profile is refused for the real config" 2 \
    "$script" --profile nope --config "$real" -- forge --version
else
  echo "skip: forge is not installed, end to end cases not run"
fi

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
