#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Checks the shared test vectors in spec/vectors/.
#
# 1. Every vector file must validate against spec/vectors/schema.json, which
#    must itself be a valid JSON Schema (draft 2020-12): amounts and u64
#    values are decimal strings, actors and labels have their forms, every
#    step and every call has exactly its fields, and so on.
# 2. scripts/check-vectors.py runs the checks a schema cannot express: rule
#    IDs, error and event names against the spec, inputs only one chain can
#    express, attestations, per-chain expectations and, when asked, coverage.
#    It runs even when step 1 fails, so one run names every problem.
#
# The files are every spec/vectors/*.json except schema.json, or the files
# given. The schema always comes from this repository; --spec points the
# rule, error and event lookups at another spec directory, which the self
# test uses.
#
# Coverage: --require-all needs every MANDATE rule that is not retired to have
# at least one passing and one failing vector. --areas A,B,... does the same
# for the rules of the given areas only, so a vector set can prove its own
# coverage. CHAINOPS rules never count as uncovered.
#
# Parity: --fixture FILE needs a vector named parity_<case> for every case of
# the attestation parity fixture FILE (spikes/attestation-parity/fixture.json),
# apart from the few cases check-vectors.py lists as beyond the attest step.
#
# Precedence: --precedence needs, for every adjacent pair of checks in every
# check order of the spec's MANDATE.md, a call marked "precedence" that fails
# both and gets the earlier check's outcome, apart from the few pairs
# check-vectors.py lists as unable to fail together. A marked call is checked
# with or without the flag.
#
# check-jsonschema is pinned, with the hash of every dependency, in
# scripts/check-vectors.requirements.txt, and the check refuses any other
# version so a local run and CI validate the same way. Install it once into a
# virtual environment of its own, then put its bin directory on PATH or name
# the binary in CHECK_JSONSCHEMA:
#
#   python3 -m venv ~/.venvs/check-jsonschema
#   ~/.venvs/check-jsonschema/bin/pip install --require-hashes --no-deps \
#     --only-binary :all: -r scripts/check-vectors.requirements.txt
#   CHECK_JSONSCHEMA=~/.venvs/check-jsonschema/bin/check-jsonschema scripts/check-vectors.sh
#
# Usage: scripts/check-vectors.sh [--require-all | --areas A,B,...] [--fixture FILE] [--precedence] [--spec DIR] [vector file...]
# Exit codes: 0 every check passes, 1 problems named on stderr, 2 bad usage,
# a missing or wrong check-jsonschema, or a spec that cannot be read.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
schema="$root/spec/vectors/schema.json"
requirements="$here/check-vectors.requirements.txt"

spec_dir="$root/spec"
mode=()
files=()
while [ $# -gt 0 ]; do
  case "$1" in
    --require-all)
      mode+=(--require-all)
      ;;
    --precedence)
      mode+=(--precedence)
      ;;
    --areas)
      if [ $# -lt 2 ]; then
        echo "check-vectors: --areas needs a list of areas, such as PAUSE,FEE" >&2
        exit 2
      fi
      mode+=(--areas "$2")
      shift
      ;;
    --spec)
      if [ $# -lt 2 ]; then
        echo "check-vectors: --spec needs a directory" >&2
        exit 2
      fi
      spec_dir=$2
      shift
      ;;
    --fixture)
      if [ $# -lt 2 ] || [ ! -f "$2" ] || [ ! -r "$2" ]; then
        echo "check-vectors: --fixture needs a readable attestation parity fixture file" >&2
        exit 2
      fi
      mode+=(--fixture "$2")
      shift
      ;;
    --)
      shift
      files+=("$@")
      break
      ;;
    -*)
      echo "check-vectors: unknown option $1" >&2
      exit 2
      ;;
    *)
      files+=("$1")
      ;;
  esac
  shift
done

if [ ! -d "$spec_dir" ]; then
  echo "check-vectors: $spec_dir is not a directory" >&2
  exit 2
fi

if [ ${#files[@]} -eq 0 ]; then
  for f in "$spec_dir"/vectors/*.json; do
    [ -e "$f" ] || continue
    [ "$(basename "$f")" = schema.json ] && continue
    files+=("$f")
  done
  if [ ${#files[@]} -eq 0 ]; then
    echo "check-vectors: no vector files in $spec_dir/vectors" >&2
    exit 2
  fi
fi
for f in "${files[@]}"; do
  if [ ! -f "$f" ] || [ ! -r "$f" ]; then
    echo "check-vectors: cannot read $f" >&2
    exit 2
  fi
done

want=$(sed -n 's/^check-jsonschema==\([0-9][0-9.]*\) .*$/\1/p' "$requirements")
if [ -z "$want" ]; then
  echo "check-vectors: no check-jsonschema pin in $requirements" >&2
  exit 2
fi
cjs=${CHECK_JSONSCHEMA:-check-jsonschema}
if ! command -v "$cjs" >/dev/null 2>&1; then
  echo "check-vectors: $cjs not found; install check-jsonschema $want as the header of this script shows" >&2
  exit 2
fi
got=$("$cjs" --version 2>/dev/null | sed -n 's/^check-jsonschema, version \(.*\)$/\1/p' || true)
if [ "$got" != "$want" ]; then
  echo "check-vectors: $cjs is version ${got:-unknown}, not the pinned $want" >&2
  exit 2
fi

status=0

# check-jsonschema checks the schema against its metaschema before it uses
# it, so a schema that is not valid JSON Schema fails here too.
echo "check-vectors: ${#files[@]} file(s) against spec/vectors/schema.json (check-jsonschema $got)"
if ! "$cjs" --schemafile "$schema" "${files[@]}"; then
  status=1
fi

# The mode array is empty without a coverage flag, and an empty array
# expands to nothing only in this form under set -u in older bash.
helper=0
python3 -I "$here/check-vectors.py" --spec "$spec_dir" ${mode[@]+"${mode[@]}"} "${files[@]}" || helper=$?
if [ "$helper" -eq 2 ]; then
  exit 2
elif [ "$helper" -ne 0 ]; then
  status=1
fi

if [ "$status" -ne 0 ]; then
  echo "check-vectors: failed, see the problems above" >&2
  exit 1
fi
echo "check-vectors: every check passed"
