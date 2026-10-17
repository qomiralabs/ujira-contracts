#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Runs a command with a clean Foundry environment and a profile that exists.
#
# Foundry lets any FOUNDRY_* (and legacy DAPP_*) environment variable override
# foundry.toml, in any case, so a stray FOUNDRY_DENY, foundry_solc or
# Foundry_Src could loosen the gate without touching a tracked file. Every one
# of them is removed before the command runs.
#
# forge also falls back to the default profile, without an error, when
# FOUNDRY_PROFILE names a profile that does not exist. This script checks the
# profile against celo/foundry.toml first and refuses an unknown one.
#
# Some overrides cannot be cleared from here, because forge reads them itself
# after this script has handed over. The script refuses to run while any of
# them exists:
#   - a .env file (or symlink) in the working directory, the config's
#     directory, or any parent of either: forge loads the first .env it finds
#     walking up from each
#   - a user-level $HOME/.foundry/foundry.toml, which forge merges in
#   - FOUNDRY_CONFIG, in any case, which points forge at another config file
#   - a foundry.toml in the working directory that is not --config
# It also refuses an inherited FOUNDRY_ or DAPP_ name that is not a valid
# shell name, since it could not be removed.
#
# This covers what forge 1.5.1 reads. It cannot stop a command that changes
# directory, or a forge release that reads a new source, so CI also checks
# its results, for example that tests actually ran.
#
# Usage: scripts/foundry-env.sh [--profile <name>] [--config <foundry.toml>] -- <command> [args]...
#   --profile  defaults to "default"
#   --config   defaults to celo/foundry.toml next to this script
# Exit codes: the command's own exit code, or 2 when the profile or the
# arguments are not valid, or the script refuses to run.
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

refuse() {
  echo "foundry-env: refusing to run: $1" >&2
  exit 2
}

if [ -z "${HOME:-}" ]; then
  refuse "HOME is not set, so the user-level Foundry config cannot be checked"
fi
user_config="$HOME/.foundry/foundry.toml"
if [ -e "$user_config" ] || [ -L "$user_config" ]; then
  refuse "$user_config exists and forge would merge it into the build config"
fi

# The physical path of the config, to compare against the working directory.
config_dir=$(cd "$(dirname "$config")" && pwd -P)
config_real="$config_dir/$(basename "$config")"
here=$(pwd -P)
if { [ -e "$here/foundry.toml" ] || [ -L "$here/foundry.toml" ]; } && [ "$here/foundry.toml" != "$config_real" ]; then
  refuse "$here/foundry.toml is not $config, and forge would read it instead"
fi

# check_dotenv <dir>: refuse if <dir> or any parent holds a .env.
check_dotenv() {
  local dir=$1
  while :; do
    if [ -e "$dir/.env" ] || [ -L "$dir/.env" ]; then
      refuse "$dir/.env exists and forge would load it after the environment is cleared"
    fi
    [ "$dir" = / ] && break
    dir=$(dirname "$dir")
  done
}
check_dotenv "$here"
check_dotenv "$config_dir"

# The names come from awk's ENVIRON, not compgen -e: bash leaves out, or
# cannot unset, a name that is not a valid identifier, but it still reaches
# forge. Captured in a variable, so an awk failure stops the script instead of
# looking like an empty environment.
names=$(awk 'BEGIN { for (name in ENVIRON) print name }')

# forge matches these prefixes in any case, so foundry_deny counts as much as
# FOUNDRY_DENY. Bracket patterns rather than ${name^^} or nocasematch, which
# bash 3.2 (macOS) does not have.
cleared=0
while IFS= read -r name; do
  case "$name" in
    [Ff][Oo][Uu][Nn][Dd][Rr][Yy]_[Cc][Oo][Nn][Ff][Ii][Gg])
      refuse "$name is set and would point forge at another config file"
      ;;
    [Ff][Oo][Uu][Nn][Dd][Rr][Yy]_* | [Dd][Aa][Pp][Pp]_*)
      if ! [[ $name =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]]; then
        refuse "inherited variable '$name' is not a valid shell name and cannot be cleared"
      fi
      unset "$name"
      cleared=$((cleared + 1))
      ;;
  esac
done <<<"$names"

if [ "$cleared" -gt 0 ]; then
  echo "foundry-env: cleared $cleared FOUNDRY_ or DAPP_ variable(s), any case" >&2
fi

export FOUNDRY_PROFILE="$profile"
exec "$@"
