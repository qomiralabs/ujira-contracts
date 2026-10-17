#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Runs a command with a clean Foundry environment and a profile that exists.
#
# Foundry lets any FOUNDRY_* (and legacy DAPP_*) environment variable override
# foundry.toml, so a stray FOUNDRY_DENY, FOUNDRY_SOLC or FOUNDRY_SRC could
# loosen the gate without touching a tracked file. Every one of them is
# removed before the command runs.
#
# forge also falls back to the default profile, without an error, when
# FOUNDRY_PROFILE names a profile that does not exist. This script checks the
# profile against celo/foundry.toml first and refuses an unknown one.
#
# Usage: scripts/foundry-env.sh [--profile <name>] [--config <foundry.toml>] -- <command> [args]...
#   --profile  defaults to "default"
#   --config   defaults to celo/foundry.toml next to this script
# Exit codes: the command's own exit code, or 2 when the profile or the
# arguments are not valid.
set -euo pipefail

profile=default
config="$(cd "$(dirname "$0")/.." && pwd)/celo/foundry.toml"

while [ $# -gt 0 ]; do
  case "$1" in
    --profile)
      [ $# -ge 2 ] || { echo "foundry-env: --profile needs a name" >&2; exit 2; }
      profile=$2
      shift 2
      ;;
    --config)
      [ $# -ge 2 ] || { echo "foundry-env: --config needs a file" >&2; exit 2; }
      config=$2
      shift 2
      ;;
    --)
      shift
      break
      ;;
    *)
      echo "foundry-env: unexpected argument '$1', put the command after --" >&2
      exit 2
      ;;
  esac
done

if [ $# -eq 0 ]; then
  echo "foundry-env: no command given" >&2
  exit 2
fi

# Profile names are plain identifiers, which also keeps the name safe to use
# in the pattern below.
if ! [[ $profile =~ ^[A-Za-z0-9_-]+$ ]]; then
  echo "foundry-env: '$profile' is not a valid profile name" >&2
  exit 2
fi

if [ ! -f "$config" ]; then
  echo "foundry-env: no foundry.toml at $config" >&2
  exit 2
fi

if ! grep -Eq "^[[:space:]]*\[profile\.\"?${profile}\"?\][[:space:]]*(#.*)?$" "$config"; then
  echo "foundry-env: profile '$profile' is not defined in $config" >&2
  exit 2
fi

cleared=0
while IFS= read -r name; do
  case "$name" in
    FOUNDRY_* | DAPP_*)
      unset "$name"
      cleared=$((cleared + 1))
      ;;
  esac
done < <(compgen -e)

if [ "$cleared" -gt 0 ]; then
  echo "foundry-env: cleared $cleared FOUNDRY_ or DAPP_ variable(s)" >&2
fi

export FOUNDRY_PROFILE="$profile"
exec "$@"
