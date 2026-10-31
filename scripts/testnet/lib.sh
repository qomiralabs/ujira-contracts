# shellcheck shell=bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Shared helpers for the scripts that talk to Stellar testnet: the deploy,
# the smoke test and the end to end checks. Source it, then call
# testnet_init before anything else.
#
# Testnet only. Every network call carries the testnet passphrase, and
# testnet_init asks the RPC server which network it serves and stops unless
# it is testnet, so a mistyped RPC URL can never sign for another network.
#
# Keys never live in the repository. The stellar CLI keeps its identities
# under $UJIRA_TESTNET_HOME/stellar (default ~/.config/ujira-testnet/stellar),
# and testnet_init refuses a home inside the repository. Nothing here prints
# a secret key: identities are created without --as-secret and only their
# public keys are read back.
#
# Environment:
#   UJIRA_TESTNET_HOME     where identities and run logs live
#   UJIRA_TESTNET_RPC_URL  another testnet RPC server, when the default one
#                          is down; it must still answer as testnet

# The constants below are read by the scripts that source this file.
# shellcheck disable=SC2034
{
TESTNET_PASSPHRASE='Test SDF Network ; September 2015'
TESTNET_RPC_DEFAULT='https://soroban-testnet.stellar.org'
TESTNET_HORIZON='https://horizon-testnet.stellar.org'

# Circle's testnet USDC. The factory lists it so the app can run against the
# same asset it will settle in, next to a test asset these scripts can mint.
TESTNET_USDC_CODE='USDC'
TESTNET_USDC_ISSUER='GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5'

# The asset the smoke and end to end tests pay in. Its issuer is a throwaway
# testnet identity, so the tests can mint whatever they need.
TEST_ASSET_CODE='UJTEST'
}

# Calls retried when the RPC server or friendbot drops a request. A call that
# fails for any other reason fails at once.
TESTNET_ATTEMPTS=${TESTNET_ATTEMPTS:-4}

testnet_die() {
  echo "testnet: $*" >&2
  exit 1
}

testnet_log() {
  echo "testnet: $*" >&2
}

# Sets up the CLI wrapper and checks the network. $1 is the repository root.
testnet_init() {
  local root=$1
  for tool in stellar python3 curl git; do
    command -v "$tool" >/dev/null 2>&1 || testnet_die "$tool is not installed"
  done

  # An inherited network, account or RPC setting would override the flags
  # below for some commands, so none is allowed through.
  unset STELLAR_NETWORK STELLAR_RPC_URL STELLAR_NETWORK_PASSPHRASE STELLAR_ACCOUNT \
    STELLAR_RPC_HEADERS SOROBAN_NETWORK SOROBAN_RPC_URL SOROBAN_NETWORK_PASSPHRASE SOROBAN_ACCOUNT

  TESTNET_HOME=${UJIRA_TESTNET_HOME:-"$HOME/.config/ujira-testnet"}
  mkdir -p "$TESTNET_HOME/stellar" "$TESTNET_HOME/logs"
  chmod 700 "$TESTNET_HOME" "$TESTNET_HOME/stellar"
  TESTNET_HOME=$(cd "$TESTNET_HOME" && pwd -P)
  local real_root
  real_root=$(cd "$root" && pwd -P)
  case "$TESTNET_HOME/" in
    "$real_root"/*) testnet_die "UJIRA_TESTNET_HOME must be outside the repository" ;;
  esac
  TESTNET_CONFIG="$TESTNET_HOME/stellar"

  TESTNET_RPC=${UJIRA_TESTNET_RPC_URL:-$TESTNET_RPC_DEFAULT}
  TESTNET_NET=(--rpc-url "$TESTNET_RPC" --network-passphrase "$TESTNET_PASSPHRASE")

  local served
  served=$(testnet_retry testnet_rpc_passphrase) || testnet_die "cannot reach $TESTNET_RPC"
  if [ "$served" != "$TESTNET_PASSPHRASE" ]; then
    testnet_die "$TESTNET_RPC serves '$served', not testnet; refusing to continue"
  fi

  TESTNET_RUN="$TESTNET_HOME/logs/$(date -u +%Y%m%dT%H%M%SZ)-$$"
  mkdir -p "$TESTNET_RUN"
  : >"$TESTNET_RUN/txs.tsv"
  testnet_log "network testnet via $TESTNET_RPC, logs in $TESTNET_RUN"
}

testnet_rpc_passphrase() {
  curl -fsS --max-time 30 -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"getNetwork"}' "$TESTNET_RPC" |
    python3 -c 'import json,sys; print(json.load(sys.stdin)["result"]["passphrase"])'
}

# The stellar CLI, always on the testnet config directory.
st() {
  stellar --config-dir "$TESTNET_CONFIG" "$@"
}

# True when the text names a failure of the transport rather than of the call.
testnet_transient() {
  grep -qiE 'timed out|timeout|connection (reset|refused|closed)|error sending request|dns error|502|503|504|too many requests|429|tx_too_late|TRY_AGAIN_LATER|transaction submission timeout|networking or low-level protocol error' "$1"
}

# Runs a command, retrying while it fails for a transient reason. Its stdout
# passes through; stderr is kept for the check and then replayed.
testnet_retry() {
  local attempt=1 err
  err=$(mktemp)
  while :; do
    if "$@" 2>"$err"; then
      # Replayed on success too: the CLI logs the transaction hash there.
      cat "$err" >&2
      rm -f "$err"
      return 0
    fi
    if [ "$attempt" -ge "$TESTNET_ATTEMPTS" ] || ! testnet_transient "$err"; then
      cat "$err" >&2
      rm -f "$err"
      return 1
    fi
    testnet_log "transient failure, retrying ($attempt of $TESTNET_ATTEMPTS): $(head -c 300 "$err" | tr '\n' ' ')"
    attempt=$((attempt + 1))
    sleep $((attempt * 5))
  done
}

# Creates the identity when missing and makes sure its account exists, which
# also covers a testnet reset. Prints the public key.
testnet_account() {
  local name=$1
  if ! st keys public-key "$name" >/dev/null 2>&1; then
    st keys generate "$name" >/dev/null 2>&1 || testnet_die "cannot create identity $name"
  fi
  local pk
  pk=$(st keys public-key "$name") || testnet_die "cannot read the public key of $name"
  if ! testnet_account_exists "$pk"; then
    testnet_retry st keys fund "$name" "${TESTNET_NET[@]}" >/dev/null ||
      testnet_die "friendbot could not fund $name ($pk)"
  fi
  echo "$pk"
}

testnet_account_exists() {
  curl -fsS --max-time 30 -o /dev/null "$TESTNET_HORIZON/accounts/$1"
}

# The balance of a classic account in CODE:ISSUER, in minor units (stroops),
# or "none" when the account holds no trustline for it.
testnet_trustline_balance() {
  local account=$1 code=$2 issuer=$3
  testnet_retry curl -fsS --max-time 30 "$TESTNET_HORIZON/accounts/$account" |
    python3 -c '
import json, sys
from decimal import Decimal
code, issuer = sys.argv[1], sys.argv[2]
for b in json.load(sys.stdin)["balances"]:
    if b.get("asset_code") == code and b.get("asset_issuer") == issuer:
        # Horizon prints 7 decimal places; turn them into integer stroops
        # without going through a float.
        print(int(Decimal(b["balance"]).scaleb(7)))
        break
else:
    print("none")
' "$code" "$issuer"
}

# Opens a trustline from the identity to CODE:ISSUER and records the hash.
testnet_trust() {
  local label=$1 name=$2 line=$3
  testnet_tx "$label" st tx new change-trust --source-account "$name" --line "$line" "${TESTNET_NET[@]}"
}

# Runs a CLI command that submits one transaction, records its hash under
# LABEL and prints the command's stdout (the return value for an invoke).
# The hash comes from the CLI's own log line, so it is the hash of the
# transaction that was signed and sent.
testnet_tx() {
  local label=$1
  shift
  local out="$TESTNET_RUN/$label.out" err="$TESTNET_RUN/$label.err"
  if ! testnet_retry "$@" >"$out" 2>"$err"; then
    cat "$err" >&2
    testnet_die "$label failed"
  fi
  local hash
  hash=$(testnet_hash_from "$err")
  if [ -z "$hash" ]; then
    # A few commands, such as wrapping an asset, send a transaction without
    # logging its hash. Callers that can live with that say so.
    if [ "${TESTNET_HASH_OPTIONAL:-0}" = 1 ]; then
      testnet_log "$label: landed, the CLI logged no hash"
      cat "$out"
      return 0
    fi
    cat "$err" >&2
    testnet_die "$label logged no transaction hash"
  fi
  printf '%s\t%s\n' "$label" "$hash" >>"$TESTNET_RUN/txs.tsv"
  testnet_log "$label: $hash"
  cat "$out"
}

# The last transaction hash a CLI run logged, or nothing. Tolerant of the
# icons and wording around it, since only the 64 hex digits matter.
testnet_hash_from() {
  grep -oE '(Signing transaction|Transaction hash is|/tx/)[: ]*[0-9a-f]{64}' "$1" |
    grep -oE '[0-9a-f]{64}' | tail -n 1
}

# A read only call: simulated, never sent.
testnet_read() {
  testnet_retry st contract invoke "${TESTNET_NET[@]}" --send=no "$@"
}

testnet_hex32() {
  python3 -c 'import secrets; print(secrets.token_hex(32))'
}

testnet_now() {
  date -u +%s
}

# Prints a field of a JSON value: testnet_json FILE_OR_DASH EXPR, where EXPR is
# a Python expression over `d`.
testnet_json() {
  local src=$1 expr=$2
  if [ "$src" = - ]; then
    python3 -c "import json,sys; d=json.load(sys.stdin); print($expr)"
  else
    python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print($expr)" "$src"
  fi
}

# Strips the quotes the CLI puts around a returned string or number.
testnet_unquote() {
  python3 -c 'import json,sys; v=json.loads(sys.stdin.read()); print(v)'
}

testnet_expect() {
  local what=$1 want=$2 got=$3
  if [ "$want" != "$got" ]; then
    testnet_die "$what: expected $want, got $got"
  fi
  testnet_log "ok: $what = $got"
}

testnet_print_txs() {
  echo "Transactions (testnet):"
  while IFS=$'\t' read -r label hash; do
    printf '  %-28s %s\n' "$label" "$hash"
  done <"$TESTNET_RUN/txs.tsv"
}
