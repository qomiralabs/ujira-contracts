#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Self test for soroban-build.sh. Each case builds a throwaway Cargo workspace
# with production crates under contracts/ and a spike crate outside it, and
# puts a fake stellar on PATH that writes the wasm a real build would. The
# real stellar build runs in CI; this tests which packages get built and what
# the release folder is allowed to hold.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
script="$here/soroban-build.sh"
work=$(cd "$(mktemp -d)" && pwd -P)
trap 'rm -rf "$work"' EXIT

passed=0
failures=0
want_out=""
reject_out=""

# expect <case name> <exit code> <command>... ; output checks come from
# $want_out and $reject_out, set by the caller for the next case only.
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

# crate <workspace> <path> <name> <crate types>
crate() {
  mkdir -p "$1/$2/src"
  printf '[package]\nname = "%s"\nversion = "0.0.0"\nedition = "2021"\npublish = false\n\n[lib]\ncrate-type = [%s]\n' \
    "$3" "$4" >"$1/$2/Cargo.toml"
  printf '#![no_std]\n' >"$1/$2/src/lib.rs"
}

# workspace <dir> <member>... writes the workspace manifest and its lockfile.
workspace() {
  local dir=$1 members=""
  shift
  for m in "$@"; do members="$members\"$m\", "; done
  mkdir -p "$dir"
  printf '[workspace]\nresolver = "2"\nmembers = [%s]\n' "$members" >"$dir/Cargo.toml"
  (cd "$dir" && cargo generate-lockfile --offline -q)
}

ws="$work/ws"
crate "$ws" contracts/mandate ujira-mandate '"cdylib", "rlib"'
crate "$ws" contracts/factory ujira-factory '"cdylib", "rlib"'
crate "$ws" contracts/shared ujira-shared '"rlib"'
crate "$ws" spikes/edges soroban-edges '"cdylib", "rlib"'
workspace "$ws" contracts/mandate contracts/factory contracts/shared spikes/edges
release="$ws/target/wasm32v1-none/release"

# A fake stellar that logs its arguments and writes the wasm for --package,
# as stellar contract build does. FAKE_EXTRA names one more wasm to write,
# FAKE_SKIP=1 writes nothing, FAKE_FAIL=1 exits 1.
mkdir -p "$work/bin"
cat >"$work/bin/stellar" <<'FAKE'
#!/usr/bin/env bash
set -euo pipefail
echo "stellar $*" >>"$FAKE_LOG"
[ "${FAKE_FAIL:-}" = 1 ] && { echo "error: build failed" >&2; exit 1; }
pkg=""
while [ $# -gt 0 ]; do
  case "$1" in
    --package) pkg=$2; shift 2 ;;
    *) shift ;;
  esac
done
dir="$PWD/target/wasm32v1-none/release"
mkdir -p "$dir"
if [ "${FAKE_SKIP:-}" != 1 ]; then
  printf 'wasm' >"$dir/$(printf '%s' "$pkg" | tr - _).wasm"
fi
if [ -n "${FAKE_EXTRA:-}" ]; then
  printf 'wasm' >"$dir/$FAKE_EXTRA.wasm"
fi
FAKE
chmod +x "$work/bin/stellar"
export FAKE_LOG="$work/stellar.log"
PATH="$work/bin:$PATH"

want_out="ujira-factory"
reject_out="soroban-edges"
expect "--list names cdylib crates under contracts/ only" 0 "$script" --list "$ws"

reject_out="ujira-shared"
expect "--list skips a library crate with no cdylib" 0 "$script" --list "$ws"

: >"$FAKE_LOG"
want_out="release folder holds exactly: ujira_factory.wasm ujira_mandate.wasm"
expect "a build makes the production wasm and nothing else" 0 "$script" "$ws"

if grep -q 'soroban-edges' "$FAKE_LOG" || grep -q 'ujira-shared' "$FAKE_LOG"; then
  echo "FAIL: only production packages are passed to stellar"
  sed 's/^/    /' "$FAKE_LOG"
  failures=$((failures + 1))
elif [ "$(grep -c -- '--locked --package' "$FAKE_LOG")" -ne 2 ]; then
  echo "FAIL: every stellar call uses --locked and one --package"
  sed 's/^/    /' "$FAKE_LOG"
  failures=$((failures + 1))
else
  echo "ok: only production packages are passed to stellar, each with --locked"
  passed=$((passed + 1))
fi

mkdir -p "$release"
printf 'stale' >"$release/soroban_edges.wasm"
expect "a stale wasm restored from a cache is removed first" 0 "$script" "$ws"
if [ -e "$release/soroban_edges.wasm" ]; then
  echo "FAIL: the stale spike wasm is still in the release folder"
  failures=$((failures + 1))
else
  echo "ok: the stale spike wasm is gone"
  passed=$((passed + 1))
fi

want_out="unexpected wasm in the release folder: soroban_edges.wasm"
FAKE_EXTRA=soroban_edges expect "a spike wasm in the release folder fails" 1 "$script" "$ws"

want_out="missing wasm in the release folder: ujira_factory.wasm"
FAKE_SKIP=1 expect "a missing production wasm fails" 1 "$script" "$ws"

want_out="stellar contract build failed for ujira-factory"
FAKE_FAIL=1 expect "a failed stellar build fails" 1 "$script" "$ws"

empty="$work/empty"
crate "$empty" spikes/only soroban-only '"cdylib"'
workspace "$empty" spikes/only
want_out="no production contracts under"
expect "a workspace with no production contract exits 2" 2 "$script" "$empty"

want_out="no Cargo.toml"
expect "a folder that is not a workspace exits 2" 2 "$script" "$work/nothing"

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
