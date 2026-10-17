#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Self test for check-sizes.sh. The budget logic is tested on captured size
# reports, then the forge path is tested end to end on throwaway projects that
# use the real celo/foundry.toml, so a forge upgrade that changes its output or
# exit codes shows up here.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
script="$here/check-sizes.sh"
config="$here/../celo/foundry.toml"
work=$(cd "$(mktemp -d)" && pwd -P)
trap 'rm -rf "$work"' EXIT

passed=0
failures=0

# expect <case name> <exit code> <args> [text the output must contain]...
expect() {
  local name=$1 want=$2 args=$3
  shift 3
  local out code=0 needle
  # shellcheck disable=SC2086
  out=$("$script" $args 2>&1) || code=$?
  if [ "$code" -ne "$want" ]; then
    echo "FAIL: $name: exit $code, want $want"
    printf '%s\n' "$out" | sed 's/^/    /'
    failures=$((failures + 1))
    return
  fi
  for needle in "$@"; do
    if ! printf '%s\n' "$out" | grep -qF -- "$needle"; then
      echo "FAIL: $name: output does not mention '$needle'"
      printf '%s\n' "$out" | sed 's/^/    /'
      failures=$((failures + 1))
      return
    fi
  done
  echo "ok: $name"
  passed=$((passed + 1))
}

# report <file name> <json>
report() {
  printf '%s\n' "$2" >"$work/$1"
}

entry() {
  printf '"%s":{"runtime_size":%s,"init_size":%s,"runtime_margin":0,"init_margin":0}' "$1" "$2" "$3"
}

# allow <file name> [entry]... writes a factory allowlist
allow() {
  local file=$1
  shift
  { echo "# test allowlist"; printf '%s\n' "$@"; } >"$work/$file"
}

uf='UjiraFactory (src/UjiraFactory.sol)'
allow factories.txt src/UjiraFactory.sol:UjiraFactory
allow none.txt
F="--factories $work/factories.txt"
N="--factories $work/none.txt"

report under.json "{$(entry UjiraMandate 24576 30000),$(entry "$uf" 61440 70000)}"
expect "both at their budget pass" 0 "$F --from-json $work/under.json" "2 contract(s) within budget" \
  "src/UjiraFactory.sol:UjiraFactory (factory)"

report mandate_over.json "{$(entry UjiraMandate 24577 30000)}"
expect "mandate one byte over fails" 1 "$N --from-json $work/mandate_over.json" \
  "UjiraMandate (contract) runtime 24577 bytes, budget 24576"

report factory_over.json "{$(entry "$uf" 61441 70000)}"
expect "factory one byte over fails" 1 "$F --from-json $work/factory_over.json" \
  "src/UjiraFactory.sol:UjiraFactory (factory) runtime 61441 bytes, budget 61440"

report factory_midsize.json "{$(entry "$uf" 25334 40000)}"
expect "allowlisted factory above EIP-170 passes" 0 "$F --from-json $work/factory_midsize.json" \
  "src/UjiraFactory.sol:UjiraFactory (factory)"

expect "the same factory without an allowlist entry fails" 1 "$N --from-json $work/factory_midsize.json" \
  "src/UjiraFactory.sol:UjiraFactory (contract) runtime 25334 bytes, budget 24576"

report renamed_mandate.json "{$(entry "$uf" 30000 31000),$(entry 'UjiraMandateFactory (src/UjiraMandate.sol)' 30000 31000)}"
expect "a mandate renamed with a Factory suffix fails" 1 "$F --from-json $work/renamed_mandate.json" \
  "src/UjiraMandate.sol:UjiraMandateFactory (contract) runtime 30000 bytes, budget 24576"

report bare_factory.json "{$(entry UjiraFactory 30000 31000)}"
expect "a bare name never gets the factory budget" 1 "$N --from-json $work/bare_factory.json" \
  "UjiraFactory (contract) runtime 30000 bytes, budget 24576"

report other_path.json "{$(entry 'UjiraFactory (src/other/UjiraFactory.sol)' 30000 31000)}"
expect "the factory name at another path fails" 1 "$N --from-json $work/other_path.json" \
  "src/other/UjiraFactory.sol:UjiraFactory (contract)"

report colon_key.json "{$(entry src/UjiraFactory.sol:UjiraFactory 30000 31000)}"
expect "a path:name key is matched by path" 0 "$F --from-json $work/colon_key.json" \
  "src/UjiraFactory.sol:UjiraFactory (factory)"

report misnamed.json "{$(entry FactoryHelper 30000 31000)}"
expect "factory only as a suffix" 1 "$N --from-json $work/misnamed.json" "FactoryHelper (contract)"

report initcode_over.json "{$(entry "$uf" 60000 131073)}"
expect "initcode over the Celo limit fails" 1 "$F --from-json $work/initcode_over.json" \
  "initcode 131073 bytes, Celo limit 131072"

report reports_all.json "{$(entry A 30000 1),$(entry BFactory 70000 1),$(entry C 10 1)}"
expect "every offending contract is named" 1 "$N --from-json $work/reports_all.json" \
  "A (contract)" "BFactory (contract)" "2 of 3 contract(s)"

# Allowlist errors
allow unused.txt src/UjiraFactory.sol:UjiraFactory src/Gone.sol:GoneFactory
expect "an allowlist entry that matches nothing is an error" 2 \
  "--factories $work/unused.txt --from-json $work/under.json" "src/Gone.sol:GoneFactory matches no contract"
allow dup.txt src/UjiraFactory.sol:UjiraFactory src/UjiraFactory.sol:UjiraFactory
expect "a duplicate allowlist entry is an error" 2 "--factories $work/dup.txt --from-json $work/under.json" \
  "listed twice"
for bad in UjiraFactory 'src/UjiraFactory.sol' '../src/X.sol:X' '/abs/X.sol:X' 'src/X.sol:1X' 'src/X.sol:X extra'; do
  allow malformed.txt "$bad"
  expect "malformed allowlist entry '$bad' is an error" 2 "--factories $work/malformed.txt --from-json $work/under.json" \
    "not of the form src/<file>.sol:<Contract>"
done
expect "a missing allowlist is an error" 2 "--factories $work/nope.txt --from-json $work/under.json" \
  "no factory allowlist"
expect "missing --factories file argument" 2 "--factories" "needs a file"
report mandate_ok.json "{$(entry UjiraMandate 100 200)}"
expect "the committed allowlist is valid" 0 "--from-json $work/mandate_ok.json" "1 contract(s) within budget"

report empty.json '{}'
expect "empty report is an error" 2 "$N --from-json $work/empty.json" "could not read contract sizes"

report garbage.json 'Compiling 3 files'
expect "non JSON report is an error" 2 "$N --from-json $work/garbage.json" "could not read contract sizes"

report reshaped.json '{"UjiraMandate":{"runtime":100}}'
expect "reshaped report is an error" 2 "$N --from-json $work/reshaped.json" "could not read contract sizes"

# Each of these once made the shell comparison error out, which read as within budget.
report fractional.json "{$(entry UjiraMandate 30000.5 31000)}"
expect "fractional runtime size is an error" 2 "$N --from-json $work/fractional.json" "could not read contract sizes"

report negative.json "{$(entry UjiraMandate -1 31000)}"
expect "negative runtime size is an error" 2 "$N --from-json $work/negative.json" "could not read contract sizes"

report negative_init.json "{$(entry "$uf" 100 -5)}"
expect "negative initcode size is an error" 2 "$N --from-json $work/negative_init.json" "could not read contract sizes"

report string_size.json "{$(entry UjiraMandate '"30000"' 31000)}"
expect "string runtime size is an error" 2 "$N --from-json $work/string_size.json" "could not read contract sizes"

report huge.json "{$(entry UjiraMandate 1e20 31000)}"
expect "size too large for shell arithmetic is an error" 2 "$N --from-json $work/huge.json" "could not read contract sizes"

report one_bad.json "{$(entry Placeholder 116 142),$(entry UjiraMandate 1.5 31000)}"
expect "one bad size among good ones is an error" 2 "$N --from-json $work/one_bad.json" "could not read contract sizes"

expect "missing --from-json file argument" 2 "--from-json" "needs a file"

# project <name> starts a throwaway Foundry project; blob <name> <contract> <bytes> adds to it
project() {
  proj="$work/$1"
  mkdir -p "$proj/src"
  cp "$config" "$proj/foundry.toml"
  echo "# no factories" >"$proj/factory-allowlist.txt"
}

# A contract whose runtime is the given number of bytes of data plus a small
# dispatcher, which is enough to push it over a budget.
blob() {
  local hex
  hex=$(head -c "$3" /dev/zero | od -An -v -tx1 | tr -d ' \n')
  cat >"$proj/src/$2.sol" <<SOL
// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs
pragma solidity 0.8.37;

contract $2 {
    function blob() external pure returns (bytes memory) {
        return hex"$hex";
    }
}
SOL
}

project forge_ok
blob forge_ok SmallMandate 100
expect "forge: small contract passes" 0 "$proj" "SmallMandate (contract)" "1 contract(s) within budget"

project forge_mandate_over
blob forge_mandate_over BigMandate 25000
expect "forge: mandate over budget fails" 1 "$proj" "BigMandate (contract)"

project forge_factory_mid
blob forge_factory_mid BigFactory 30000
echo "src/BigFactory.sol:BigFactory" >"$proj/factory-allowlist.txt"
expect "forge: allowlisted factory over EIP-170 but within budget passes" 0 "$proj" \
  "src/BigFactory.sol:BigFactory (factory)"

project forge_fake_factory
blob forge_fake_factory FakeFactory 30000
expect "forge: FakeFactory of 30 KB outside the allowlist fails" 1 "$proj" \
  "src/FakeFactory.sol:FakeFactory (contract) runtime"

project forge_factory_over
blob forge_factory_over HugeFactory 62000
echo "src/HugeFactory.sol:HugeFactory" >"$proj/factory-allowlist.txt"
expect "forge: allowlisted factory over budget fails" 1 "$proj" "src/HugeFactory.sol:HugeFactory (factory)"

# Two contracts with the same name: forge keys them by name and path.
project forge_same_name
mkdir -p "$proj/src/a" "$proj/src/b"
blob forge_same_name TwinFactory 30000
mv "$proj/src/TwinFactory.sol" "$proj/src/a/TwinFactory.sol"
sed 's/return hex"/return hex"01/' "$proj/src/a/TwinFactory.sol" >"$proj/src/b/TwinFactory.sol"
echo "src/a/TwinFactory.sol:TwinFactory" >"$proj/factory-allowlist.txt"
expect "forge: same name at an unlisted path fails" 1 "$proj" \
  "src/a/TwinFactory.sol:TwinFactory (factory)" "src/b/TwinFactory.sol:TwinFactory (contract)"

project forge_stale_entry
blob forge_stale_entry SmallMandate 100
echo "src/Gone.sol:GoneFactory" >"$proj/factory-allowlist.txt"
expect "forge: allowlist entry for a missing contract is an error" 2 "$proj" "matches no contract"

project forge_broken
printf '// SPDX-License-Identifier: Apache-2.0\npragma solidity 0.8.37;\ncontract Broken {\n' >"$proj/src/Broken.sol"
expect "forge: compile error exits 2" 2 "$proj" "forge build failed"

project forge_warning
cat >"$proj/src/Warns.sol" <<'SOL'
// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs
pragma solidity 0.8.37;

contract Warns {
    function f() external pure returns (uint256) {
        uint256 unused = 2;
        return 1;
    }
}
SOL
expect "forge: denied warning exits 2" 2 "$proj" "forge build failed"

mkdir -p "$work/no_project"
echo "# none" >"$work/no_project/factory-allowlist.txt"
expect "no foundry.toml exits 2" 2 "$work/no_project" "no foundry.toml"

echo
echo "$passed passed, $failures failed"
[ "$failures" -eq 0 ]
