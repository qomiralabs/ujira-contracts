#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Self test for check-workflow-downloads.sh, including that the flags the workflows
# use really do retry a 504, and really do give up on one that never clears.
set -euo pipefail

check="$(cd "$(dirname "$0")" && pwd)/check-workflow-downloads.sh"
work=$(mktemp -d)
server=
cleanup() {
  [ -z "$server" ] || kill "$server" 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT

passed=0
failed=0
ok() {
  passed=$((passed + 1))
  echo "ok: $1"
}
bad() {
  failed=$((failed + 1))
  echo "FAILED: $1"
}

# Kept in step with the workflows by the first case below, which reads them.
FLAGS='-fsSL --retry 5 --retry-delay 5 --retry-all-errors --connect-timeout 20 --max-time 180'

expect() {
  local name=$1 code=$2 pattern=$3 dir=$4 output rc=0
  output=$("$check" "$dir" 2>&1) || rc=$?
  if [ "$rc" = "$code" ] && grep -q -- "$pattern" <<<"$output"; then
    ok "$name"
  else
    bad "$name (exit $rc): $output"
  fi
}

case_dir() {
  local dir="$work/$1"
  mkdir -p "$dir"
  cat >"$dir/ci.yml"
  echo "$dir"
}

expect 'the real workflows keep the rules' 0 'retry and check their checksum' \
  "$(cd "$(dirname "$0")/.." && pwd)/.github/workflows"

grep -q -- "curl $FLAGS -o" "$(dirname "$0")/../.github/workflows/ci.yml" &&
  ok 'this test exercises the flags ci.yml uses' ||
  bad 'ci.yml does not use the flags this test exercises'

expect 'a download with retries and a checksum passes' 0 '1 download(s)' "$(case_dir good <<YAML
        run: |
          curl $FLAGS -o tool.tgz "https://example.invalid/tool.tgz"
          echo "\${SHA}  tool.tgz" | sha256sum --check --strict
YAML
)"

expect 'sha512 counts as a checksum' 0 '1 download(s)' "$(case_dir sha512 <<YAML
        run: |
          curl $FLAGS -o tool.tgz "https://example.invalid/tool.tgz"
          echo "\${SHA}  tool.tgz" | sha512sum --check --strict
YAML
)"

expect 'a download with no retries fails' 1 'has no --retry$' "$(case_dir bare <<'YAML'
        run: |
          curl -fsSLo tool.tgz "https://example.invalid/tool.tgz"
          echo "${SHA}  tool.tgz" | sha256sum --check --strict
YAML
)"

for flag in --retry-delay --retry-all-errors --connect-timeout --max-time; do
  flags=$(sed -E "s/ $flag( [0-9]+)?//" <<<"$FLAGS")
  expect "a download missing $flag fails" 1 "has no $flag" "$(case_dir "missing$flag" <<YAML
        run: |
          curl $flags -o tool.tgz "https://example.invalid/tool.tgz"
          echo "\${SHA}  tool.tgz" | sha256sum --check --strict
YAML
)"
done

expect 'a download with no checksum check fails' 1 'no checksum check' "$(case_dir nosum <<YAML
        run: |
          curl $FLAGS -o tool.tgz "https://example.invalid/tool.tgz"
          tar -xzf tool.tgz
YAML
)"

expect 'a checksum too far from its download fails' 1 'no checksum check' "$(case_dir far <<YAML
        run: |
          curl $FLAGS -o tool.tgz "https://example.invalid/tool.tgz"
          echo one
          echo two
          echo three
          echo "\${SHA}  tool.tgz" | sha256sum --check --strict
YAML
)"

expect 'a comment that mentions curl is not a download' 0 '0 download(s)' "$(case_dir comment <<'YAML'
        run: |
          # curl would go here
          echo nothing
YAML
)"

# A real server, because whether -f, --retry and a 504 work together is what failed.
cat >"$work/server.py" <<'PY'
import http.server, sys
remaining = {'/flaky': 2, '/down': 10**9}
hits = {}
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        hits[self.path] = hits.get(self.path, 0) + 1
        open(sys.argv[1] + self.path.replace('/', '_'), 'w').write(str(hits[self.path]))
        if remaining.get(self.path, 0) > 0:
            remaining[self.path] -= 1
            self.send_response(504); self.end_headers(); return
        self.send_response(200); self.end_headers(); self.wfile.write(b'the tool\n')
    def log_message(self, *args): pass
server = http.server.HTTPServer(('127.0.0.1', 0), Handler)
open(sys.argv[1] + 'port', 'w').write(str(server.server_port))
server.serve_forever()
PY
python3 "$work/server.py" "$work/" &
server=$!
for _ in $(seq 1 50); do [ -s "$work/port" ] && break; sleep 0.1; done
port=$(cat "$work/port")

# The same flags, with shorter waits, so the test does not take half a minute.
fast=$(sed -E 's/--retry-delay [0-9]+/--retry-delay 1/' <<<"$FLAGS")

# shellcheck disable=SC2086
if curl $fast -o "$work/flaky.out" "http://127.0.0.1:$port/flaky" 2>/dev/null &&
  [ "$(cat "$work/_flaky")" = 3 ] && [ "$(cat "$work/flaky.out")" = 'the tool' ]; then
  ok 'a 504 twice, then success: retried, and the file arrives'
else
  bad "a 504 was not retried to success: hits $(cat "$work/_flaky" 2>/dev/null)"
fi

few=$(sed -E 's/--retry 5/--retry 2/' <<<"$fast")
rc=0
# shellcheck disable=SC2086
curl $few -o "$work/down.out" "http://127.0.0.1:$port/down" 2>/dev/null || rc=$?
if [ "$rc" = 22 ] && [ "$(cat "$work/_down")" = 3 ]; then
  ok 'a 504 that never clears: gives up after its retries, and fails'
else
  bad "a 504 that never clears: exit $rc, hits $(cat "$work/_down" 2>/dev/null)"
fi

# What the workflows do next, on what a retry might bring back.
echo 'not the tool' >"$work/wrong.out"
want=$(printf 'the tool\n' | sha256sum | cut -d' ' -f1)
if ! (cd "$work" && echo "$want  wrong.out" | sha256sum --check --strict >/dev/null 2>&1); then
  ok 'a wrong file still fails the checksum'
else
  bad 'a wrong file passed the checksum'
fi

echo
echo "$passed passed, $failed failed"
[ "$failed" -eq 0 ]
