#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Checks the rule IDs in spec/MANDATE.md.
#
# A rule is a level four heading that starts with its ID:
#
#   #### MANDATE-<AREA>-<NNN> Title
#   #### CHAINOPS-<AREA>-<NNN> Title
#
# Every MANDATE rule must carry the three fields "- Precondition:",
# "- Effect:" and "- Error:" before the next heading, and every error name it
# cites in backticks on its Error line must be listed in the error table (a
# "## Errors" section whose rows read "| <code> | `NAME` | ..."). Every
# CHAINOPS rule must carry an "- Off chain:" field; those rules bind services,
# not the contracts, and are reported as excluded from vector coverage.
#
# The check fails when the error table lists a code or a name twice, when an
# ID is defined twice, when an ID is defined but not in the rule index (a "## Rule index" section with rows "| `ID` | ... |"), when
# the index lists an ID twice or lists one that is not defined, when any ID
# cited anywhere is not defined, when a field is missing, when an error name is
# unknown, or when a required rule area has no rule.
#
# With --check-order it also checks the check order blocks, the lines
# between "<!-- check-order: <name> -->" and "<!-- /check-order -->". Every
# "### <name>" operation heading under "## Operations and check orders",
# except "Views", must be followed by exactly one block of the same name, and
# no block may sit anywhere else. A block holds only numbered checks, numbered
# 1, 2, 3 in order (a letter suffix such as "9a. " follows its step). Each
# check must cite exactly one MANDATE rule ID, that rule must be defined, and
# it must be able to fail: its Error line may not start with "none". A rule
# that hands off to per-item checks says so with "- Error: per item".
# A step may not cite a rule whose precondition cites, directly or through
# rules that no step of the same order checks, a rule that another step of the
# same order checks, since the first error would then depend on how an
# implementation read it. A precondition runs from "- Precondition:" to the
# next field line, blank line or heading, whatever the indentation. Rules that
# only mention another rule do so in their Effect. Rule headings must run in
# numeric order per area.
#
# Check ranges: a phrase such as "`create` checks 3 to 16", or a bare
# "Checks 18 to 23" under the same heading as the order it follows, must name
# steps that exist; "check", "step" and "N through M", "N-M" and "N and M"
# are read the same way.
#
# Formatting: once the fields of a rule start, a blank line may only be
# followed by a heading, so every field is contiguous and nothing can hide
# after a blank line. Two blank lines in a row are refused, every heading
# needs a blank line before it.
#
# Files: the check covers what git tracks under the spec's directory (git
# ls-files --stage), so untracked files such as .DS_Store are ignored and
# nothing in the working tree can hide a file from it. Content is read from
# the staged blobs in the object store (git cat-file --batch), which is what a
# commit and GitHub serve; the working tree copy must be byte for byte equal
# to the blob, so stage an edit (git add) before running the check on it. No
# attribute that converts content between blob and checkout may apply to a
# spec path (working-tree-encoding, filter, eol, text, crlf, ident), whatever
# file sets it, and no .gitattributes may be tracked inside the spec
# directory. Inherited GIT_* variables are dropped before git is called, so
# the check always reads the repository that holds the spec. Every tracked entry must
# be a regular file (mode 100644 or 100755); a symbolic link (120000), to a
# file or a directory, or a submodule fails, as does a tracked path that is a
# link or missing in the working tree. Every tracked path must be printable
# ASCII. The spec file itself must be tracked. Without git, or outside a
# repository, the check exits 2.
#
# Character set: every tracked file in the spec's directory and its subfolders
# may hold only printable ASCII (0x20 to 0x7E) and LF. A carriage return, a tab,
# any other control character, invalid UTF-8 or any other code point fails
# with the file, line, column and code point, so nothing that renders like
# ASCII (a look-alike hyphen or letter, a no break space, a bare carriage
# return hiding a step) can pass. The allowlist below names the only
# exceptions; add one only with a reason in review.
#
# Everything inside fenced code blocks is ignored. An ID whose number does not
# have exactly three digits, or that runs on into letters (MANDATE-SWEEP-004a),
# is reported, never truncated. Letter suffixes on steps run a, b, c in order.
# An empty "- Error:" line is reported; a rule that adds no failure writes
# "- Error: none".
#
# Retired rules: a rule whose title starts with "Retired:" keeps its ID so it
# is never reused. Its Error line must be "none", and it may be cited only in
# the rule index and inside its own body, never in a check order or another
# rule. Every retired ID is listed in retired-ids.txt beside the spec (one ID
# per line, # starts a comment), and the check fails if a listed ID is missing
# or no longer titled "Retired:", or if a retired rule is not listed, so a
# retired ID can be neither deleted nor brought back.
#
# Tables: errors.json, constants.json and EVENTS.md beside the spec must
# agree with MANDATE.md. errors.json is a list of { name, code, summary } with
# unique names and codes, and its (code, name) pairs equal the error table.
# constants.json holds "constants" (equal to the constants table, I128_MAX as
# a decimal string equal to 2^127 - 1), "role_codes" and
# "recovery_action_kinds". EVENTS.md lists every event of the event list with
# the same fields, and every event MANDATE.md cites (`name {`, "emit `name`")
# is in that list. Duplicate JSON keys are refused. The three files are
# required when the checker runs on the repository's own spec (no path given)
# or with --require-tables, and checked whenever they are present.
#
# Usage: scripts/check-spec-ids.sh [--check-order] [--require-tables] [path to MANDATE.md]
# Exit codes: 0 all checks pass, 1 one or more problems named on stderr,
# 2 bad usage or an unreadable file.
set -euo pipefail

check_order=0
require_tables=0
spec=""
for arg in "$@"; do
  case "$arg" in
    --check-order) check_order=1 ;;
    --require-tables) require_tables=1 ;;
    -*)
      echo "check-spec-ids: unknown option $arg" >&2
      exit 2
      ;;
    *)
      if [ -n "$spec" ]; then
        echo "check-spec-ids: more than one file given" >&2
        exit 2
      fi
      spec=$arg
      ;;
  esac
done
if [ -z "$spec" ]; then require_tables=1; fi
spec=${spec:-"$(cd "$(dirname "$0")/.." && pwd)/spec/MANDATE.md"}

if [ ! -r "$spec" ]; then
  echo "check-spec-ids: cannot read $spec" >&2
  exit 2
fi

# The areas the spec must cover. Keep in step with the spec's own list.
mandate_areas="CONFIG ROLE AMOUNT FUND ASSET DUST PAYOUT BATCH BUDGET PAYMENT DAILY RECIPIENT SPLIT APPROVAL EXPIRY PAUSE REVOKE WITHDRAW SWEEP RECOVER RECOVERYSET PRINCIPAL EVENT FEE"
chainops_areas="XCHAIN DELIVERY SUBMIT CREATE SWEEP FEE INDEX"

# Characters allowed beyond printable ASCII and LF, per file relative to the
# spec directory. U+2016 (DOUBLE VERTICAL LINE) is the byte concatenation
# operator in MANDATE.md (the deploy salt) and in ENCODING.md (the ID and
# attestation message layouts).
charset_allow="MANDATE.md:2016 ENCODING.md:2016"

spec_dir=$(dirname "$spec")
charset_status=0
python3 -I - "$spec_dir" "$charset_allow" "$(basename "$spec")" "$require_tables" <<'PY' || charset_status=$?
import os
import subprocess
import sys
import unicodedata

root, allow_spec, spec_name, require_tables = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4] == "1"
texts = {}
allowed = {}
for item in allow_spec.split():
    name, _, points = item.partition(":")
    allowed[name] = {int(p, 16) for p in points.split(",")}

LIMIT = 20  # findings reported per file before the rest are only counted
problems = 0


def report(msg):
    global problems
    print("check-spec-ids: " + msg, file=sys.stderr)
    problems += 1


REGULAR = {b"100644", b"100755"}


def show(path):
    """A tracked path as text, with every byte outside printable ASCII escaped."""
    return "".join(chr(c) if 0x20 <= c <= 0x7E and c != 0x5C else "\\x%02x" % c for c in path)


# Every GIT_* variable is dropped so GIT_DIR, GIT_WORK_TREE, GIT_INDEX_FILE or
# injected config cannot point git at another repository or change its view.
GIT_ENV = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}

# Attributes that change bytes between the blob and the checkout. An unset
# value (for example "-text") converts nothing and is allowed.
CONVERTING = {b"working-tree-encoding", b"filter", b"eol", b"text", b"crlf", b"ident"}


def git(args, data=None):
    try:
        out = subprocess.run(["git", "-C", root] + args, input=data, env=GIT_ENV,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    except OSError:
        print("check-spec-ids: git is not available; the checks need the tracked file list", file=sys.stderr)
        sys.exit(2)
    if out.returncode != 0:
        print("check-spec-ids: %s is not in a git repository; the checks need the tracked file list" % root, file=sys.stderr)
        sys.exit(2)
    return out.stdout


def tracked_entries():
    """Returns [(mode, object id, path bytes)] for every entry git tracks under root."""
    seen = {}
    for record in git(["ls-files", "-z", "--stage", "--", "."]).split(b"\0"):
        if not record:
            continue
        meta, _, path = record.partition(b"\t")
        fields = meta.split(b" ")
        seen.setdefault(path, (fields[0], fields[1]))
    return sorted((mode, oid, path) for path, (mode, oid) in seen.items())


def read_blobs(oids):
    """Returns {object id: bytes} read from the object store.

    git cat-file --batch answers one record per requested id, in order. Every
    record must name the id that was asked for and carry its full size, so a
    wrong, missing or truncated answer can never be checked in place of the
    real object.
    """
    out = git(["cat-file", "--batch"], b"".join(o + b"\n" for o in oids))
    blobs, pos = {}, 0
    for oid in oids:
        name = oid.decode("ascii", "replace")
        end = out.find(b"\n", pos)
        if end < 0:
            fail_read("git cat-file output ended before the record for object %s" % name)
        header = out[pos:end].split(b" ")
        if len(header) != 3 or header[1] != b"blob" or not header[2].isdigit():
            fail_read("cannot read object %s from git (got %r)" % (name, out[pos:end][:80]))
        if header[0] != oid:
            fail_read("git cat-file returned object %s where %s was requested" % (header[0].decode("ascii", "replace"), name))
        size = int(header[2])
        if end + 1 + size + 1 > len(out) or out[end + 1 + size:end + 2 + size] != b"\n":
            fail_read("git cat-file record for object %s was cut short" % name)
        blobs[oid] = out[end + 1:end + 1 + size]
        pos = end + 1 + size + 1
    if pos != len(out):
        fail_read("git cat-file returned more records than were requested")
    return blobs


def fail_read(msg):
    print("check-spec-ids: " + msg, file=sys.stderr)
    sys.exit(2)


def converting_attributes(paths):
    """Returns {path: [(attribute, value)]} for attributes that convert content."""
    if not paths:
        return {}
    out = git(["check-attr", "-z", "-a", "--stdin"], b"".join(p + b"\0" for p in paths))
    parts = out.split(b"\0")
    found = {}
    for i in range(0, len(parts) - 2, 3):
        path, attr, value = parts[i], parts[i + 1], parts[i + 2]
        if attr in CONVERTING and value != b"unset":
            found.setdefault(path, []).append((attr.decode("ascii", "replace"), value.decode("ascii", "replace")))
    return found


def check_text(rel, text):
    ok = allowed.get(rel, set())
    found = 0
    line, col = 1, 0
    for i, ch in enumerate(text):
        col += 1
        cp = ord(ch)
        if ch == "\n":
            line, col = line + 1, 0
            continue
        if 0x20 <= cp <= 0x7E or cp in ok:
            continue
        found += 1
        if found > LIMIT:
            continue
        where = "%s:%d:%d" % (rel, line, col)
        if ch == "\r" and text[i + 1:i + 2] == "\n":
            report("%s:%d ends with a carriage return; line endings must be LF" % (rel, line))
        elif ch == "\r":
            report(where + " has a bare carriage return (U+000D); line endings must be LF")
        elif 0xDC80 <= cp <= 0xDCFF:
            report(where + " has the byte 0x%02X, which is not valid UTF-8" % (cp - 0xDC00))
        else:
            name = unicodedata.name(ch, "")
            label = name if name else "(a control character)" if unicodedata.category(ch) == "Cc" else "(no name)"
            report("%s has U+%04X %s; spec files hold printable ASCII and LF only" % (where, cp, label))
    if found > LIMIT:
        report("%s has %d more character(s) outside the allowed set" % (rel, found - LIMIT))


def scan():
    entries = tracked_entries()
    if spec_name.encode() not in {path for _, _, path in entries}:
        report(spec_name + " is not tracked by git; add it so these checks cover it")
    regular = []
    for mode, oid, path in entries:
        bad = [i for i, c in enumerate(path) if not 0x20 <= c <= 0x7E]
        if bad:
            report("path %s has byte 0x%02X at position %d; spec paths hold printable ASCII only"
                   % (show(path), path[bad[0]], bad[0] + 1))
            continue
        rel = path.decode("ascii")
        if mode == b"120000":
            report(rel + " is a symbolic link (mode 120000); spec files must be regular files")
            continue
        if mode not in REGULAR:
            report("%s has mode %s; spec files must be regular files (100644 or 100755)" % (rel, mode.decode("ascii", "replace")))
            continue
        if os.path.basename(rel) == ".gitattributes":
            report(rel + " is a tracked attributes file inside the spec directory; spec files may not carry attributes")
        regular.append((oid, path, rel))
    attrs = converting_attributes([path for _, path, _ in regular])
    blobs = read_blobs([oid for oid, _, _ in regular])
    for oid, path, rel in regular:
        for attr, value in attrs.get(path, []):
            report("%s has attribute %s=%s, which converts content between the blob and the checkout" % (rel, attr, value))
        blob = blobs[oid]
        texts[rel] = blob.decode("utf-8", errors="surrogateescape")
        check_text(rel, texts[rel])
        full = os.path.join(root, rel)
        if os.path.islink(full):
            report(rel + " is a symbolic link in the working tree; spec files must be regular files")
            continue
        if not os.path.isfile(full):
            report(rel + " is tracked but missing from the working tree or not a regular file")
            continue
        with open(full, "rb") as fh:
            if fh.read() != blob:
                report(rel + " in the working tree differs from the staged blob; stage the change (git add) or restore the file")


import json
import re

TABLE_FILES = ("errors.json", "constants.json", "EVENTS.md")
# What the Data column says for an event whose only field is its topic key.
EMPTY_MAP = "an empty map"


def reject_duplicates(name):
    def hook(pairs):
        seen = {}
        for key, value in pairs:
            if key in seen:
                report("%s has the key %s twice" % (name, key))
            seen[key] = value
        return seen
    return hook


def load_json(name):
    try:
        return json.loads(texts[name], object_pairs_hook=reject_duplicates(name))
    except ValueError as err:
        report("%s is not valid JSON: %s" % (name, err))
        return None


def sections(text):
    """Returns {heading text: [lines]} for every ## and ### heading."""
    out, current = {}, None
    for line in text.split("\n"):
        m = re.match(r"^#{2,3} (.+)$", line)
        if m:
            current = m.group(1).strip()
            out.setdefault(current, [])
        elif current is not None:
            out[current].append(line)
    return out


def table_rows(lines):
    rows = []
    for line in lines:
        if line.startswith("|") and not re.match(r"^\| *-", line):
            rows.append([c.strip() for c in line.strip().strip("|").split("|")])
    return rows[1:] if rows else []


def check_errors(spec_sections):
    data = load_json("errors.json")
    if data is None:
        return
    if not isinstance(data, list):
        report("errors.json must be a list of { name, code, summary }")
        return
    names, codes = {}, {}
    for i, entry in enumerate(data):
        if not isinstance(entry, dict) or set(entry) != {"name", "code", "summary"}:
            report("errors.json entry %d must have exactly name, code and summary" % (i + 1))
            continue
        name, code, summary = entry["name"], entry["code"], entry["summary"]
        if not isinstance(name, str) or not re.match(r"^[A-Z][A-Z0-9_]*$", name):
            report("errors.json entry %d has a name that is not upper snake case" % (i + 1))
            continue
        if not isinstance(code, int) or isinstance(code, bool) or code < 1:
            report("errors.json %s has a code that is not a positive integer" % name)
            continue
        if not isinstance(summary, str) or not summary.strip():
            report("errors.json %s has an empty summary" % name)
        if name in names:
            report("errors.json lists %s twice" % name)
        if code in codes:
            report("errors.json lists code %d twice" % code)
        names[name], codes[code] = code, name
    table = {}
    for row in table_rows(spec_sections.get("Errors", [])):
        if len(row) >= 2 and row[0].isdigit():
            table[row[1].strip("`")] = int(row[0])
    for name, code in sorted(table.items(), key=lambda kv: kv[1]):
        if names.get(name) != code:
            report("error %s (%d) is in the MANDATE.md error table but not in errors.json" % (name, code))
    for name, code in sorted(names.items(), key=lambda kv: kv[1]):
        if table.get(name) != code:
            report("error %s (%d) is in errors.json but not in the MANDATE.md error table" % (name, code))


def check_constants(spec_sections):
    data = load_json("constants.json")
    if data is None:
        return
    if not isinstance(data, dict) or set(data) != {"constants", "role_codes", "recovery_action_kinds"}:
        report("constants.json must have exactly constants, role_codes and recovery_action_kinds")
        return
    consts = data["constants"]
    for key in ("role_codes", "recovery_action_kinds"):
        group = data[key]
        if not isinstance(group, dict) or any(not isinstance(v, int) or isinstance(v, bool) for v in group.values()):
            report("constants.json %s must map names to integers" % key)
        elif len(set(group.values())) != len(group):
            report("constants.json %s gives two names the same code" % key)
    if not isinstance(consts, dict):
        report("constants.json constants must be an object")
        return
    table = {}
    for row in table_rows(spec_sections.get("Constants", [])):
        if len(row) >= 2 and row[0].startswith("`"):
            table[row[0].strip("`")] = row[1]
    for name, value in consts.items():
        if isinstance(value, bool) or not isinstance(value, (int, str)) or (isinstance(value, str) and not value.isdigit()):
            report("constant %s in constants.json is not an integer or a decimal string" % name)
    for name, value in table.items():
        if name not in consts:
            report("constant %s is in the MANDATE.md constants table but not in constants.json" % name)
        elif str(consts[name]) != value:
            report("constant %s is %s in MANDATE.md but %s in constants.json" % (name, value, consts[name]))
    for name in consts:
        if name not in table:
            report("constant %s is in constants.json but not in the MANDATE.md constants table" % name)
    if "I128_MAX" in consts and str(consts["I128_MAX"]) != str(2 ** 127 - 1):
        report("constant I128_MAX in constants.json is not 2^127 - 1")


def event_rows(lines):
    events = {}
    for row in table_rows(lines):
        if len(row) >= 2 and row[0].startswith("`"):
            fields = [f.strip() for f in row[1].split(",") if f.strip()]
            for name in re.findall(r"`([a-z][a-z0-9_]*)`", row[0]):
                events[name] = fields
    return events


def check_events(spec_text, spec_sections):
    listed = event_rows(spec_sections.get("Event list", []))
    documented = event_rows(texts["EVENTS.md"].split("\n"))
    for name, fields in sorted(listed.items()):
        if name not in documented:
            report("event %s is in the MANDATE.md event list but not in EVENTS.md" % name)
        elif documented[name] != fields:
            report("event %s has fields %s in MANDATE.md but %s in EVENTS.md" % (name, ", ".join(fields), ", ".join(documented[name])))
    for name in sorted(documented):
        if name not in listed:
            report("event %s is in EVENTS.md but not in the MANDATE.md event list" % name)
    # The topic key travels only in the topics: the data column lists the
    # event's fields without it. An event whose only field is its key still
    # carries data on chain, an empty map, and a decoder written from a table
    # that says "none" could expect no data at all, so the cell must read
    # "an empty map" there and nowhere else.
    for row in table_rows(texts["EVENTS.md"].split("\n")):
        if len(row) < 4 or not row[0].startswith("`"):
            continue
        fields = [f.strip() for f in row[1].split(",") if f.strip()]
        key = row[2].strip("`").strip()
        cell = row[3].strip()
        data = [f.strip() for f in cell.split(",") if f.strip()]
        for name in re.findall(r"`([a-z][a-z0-9_]*)`", row[0]):
            # tenant_ref is the operator's ID for a principal and project.
            # Published, it would let anyone group every mandate of one
            # tenant; events carry the per-mandate tenant_tag instead.
            if key == "tenant_ref" or "tenant_ref" in fields:
                report("event %s publishes tenant_ref, which links every mandate of one tenant; use the per-mandate tenant_tag" % name)
            if not [f for f in fields if f != key]:
                if cell != EMPTY_MAP:
                    report("event %s carries only its topic key, so its Data must read %s, not %s" % (name, EMPTY_MAP, cell or "an empty cell"))
            elif key in data:
                report("event %s repeats its topic key %s in the data; the key is carried only in the topics" % (name, key))
            elif data != [f for f in fields if f != key]:
                report("event %s has data %s, but its fields without the topic key are %s" % (name, ", ".join(data) or "none", ", ".join(f for f in fields if f != key) or "none"))
    fence = False
    for n, line in enumerate(spec_text.split("\n"), 1):
        if line.startswith("```"):
            fence = not fence
        if fence or line.startswith("|"):
            continue
        cited = re.findall(r"`([a-z][a-z0-9_]*) \{", line) + re.findall(r"[Ee]mits? `([a-z][a-z0-9_]*)`", line)
        for name in cited:
            if name not in listed:
                report("event %s is cited at line %d of %s but is not in the event list" % (name, n, spec_name))


def check_tables():
    present = [name for name in TABLE_FILES if name in texts]
    if require_tables:
        for name in TABLE_FILES:
            if name not in texts:
                report(name + " is missing; it must sit beside the spec and be tracked")
    if not present or spec_name not in texts:
        return
    spec_text = texts[spec_name]
    spec_sections = sections(spec_text)
    before = problems
    if "errors.json" in texts:
        check_errors(spec_sections)
    if "constants.json" in texts:
        check_constants(spec_sections)
    if "EVENTS.md" in texts:
        check_events(spec_text, spec_sections)
    if problems == before and len(present) == len(TABLE_FILES):
        print("error table, constants and events agree with %s" % spec_name)


try:
    scan()
    check_tables()
except OSError as err:
    print("check-spec-ids: cannot read the spec directory: %s" % err, file=sys.stderr)
    sys.exit(2)
sys.exit(1 if problems else 0)
PY
if [ "$charset_status" -gt 1 ]; then
  echo "check-spec-ids: the character set check could not run" >&2
  exit 2
fi

# Retired rule IDs committed beside the spec.
retired_file="$spec_dir/retired-ids.txt"
retired_list=""
retired_status=0
if [ -f "$retired_file" ]; then
  lineno=0
  while IFS= read -r entry || [ -n "$entry" ]; do
    lineno=$((lineno + 1))
    entry=${entry%%#*}
    entry=$(printf '%s' "$entry" | tr -d ' ')
    [ -z "$entry" ] && continue
    if ! printf '%s\n' "$entry" | grep -Eq '^(MANDATE|CHAINOPS)-[A-Z]+-[0-9]{3}$'; then
      echo "check-spec-ids: retired-ids.txt line $lineno is not a rule ID: $entry" >&2
      retired_status=1
      continue
    fi
    retired_list="$retired_list $entry"
  done <"$retired_file"
fi

set +e
LC_ALL=C awk \
  -v check_order="$check_order" \
  -v retired_list="$retired_list" \
  -v mandate_areas="$mandate_areas" \
  -v chainops_areas="$chainops_areas" '
function problem(msg) {
  print "check-spec-ids: " msg > "/dev/stderr"
  problems++
}

# Closes the rule that was open, checking its fields.
function close_rule() {
  if (current == "") return
  if (current ~ /^MANDATE-/) {
    if (!has_pre) problem(current " has no \"- Precondition:\" field")
    if (!has_eff) problem(current " has no \"- Effect:\" field")
    if (!has_err) problem(current " has no \"- Error:\" field")
  } else if (!has_off) {
    problem(current " has no \"- Off chain:\" field")
  }
  current = ""
}

# Returns the well formed rule IDs in s, separated by spaces, and reports any
# ID-like token whose number does not have exactly three digits.
function ids_in(s, where,    out, tok, digits) {
  out = ""
  while (match(s, tok_re)) {
    tok = substr(s, RSTART, RLENGTH)
    s = substr(s, RSTART + RLENGTH)
    digits = tok
    sub(/^[A-Z]+-[A-Z]+-/, "", digits)
    if (digits !~ /^[0-9][0-9][0-9]$/) {
      problem("malformed rule ID " tok " at line " where ", the number must be exactly three digits")
      continue
    }
    out = out " " tok
  }
  return out
}

BEGIN {
  tok_re = "(MANDATE|CHAINOPS)-[A-Z]+-[0-9]+[A-Za-z0-9_]*"
  ops_section = "Operations and check orders"
  section = ""
  in_order = 0
  in_fence = 0
  op = ""
  prev_blank = 0
}

{
  line = $0
  # Carriage returns are reported by the character set check; strip a
  # trailing one so the rest of the line still parses.
  if (line ~ /\r$/) {
    sub(/\r$/, "", line)
    $0 = line
  }
  is_blank = (line ~ /^[ \t]*$/)
  if (!in_fence) {
    if (is_blank && prev_blank) problem("two blank lines in a row at line " NR)
    if (!is_blank && line ~ /^#+ / && NR > 1 && !prev_blank) problem("heading at line " NR " needs a blank line before it")
    # The fields of a rule are contiguous: once they start, a blank line may
    # only be followed by another heading.
    if (current != "" && fields_started) {
      if (is_blank) pending_blank = 1
      else if (pending_blank && line !~ /^#+ /) {
        problem(current " has content after a blank line at line " NR "; the fields of a rule must be contiguous")
        pending_blank = 0
      }
    }
  }
  prev_blank = is_blank
}

# Fenced code blocks are examples, not spec text: nothing inside counts.
/^```/ { in_fence = !in_fence; next }
in_fence { next }

# Section tracking: level two headings name the section.
/^## / {
  close_rule()
  section = substr(line, 4)
  op = ""
}

# Operation headings under the operations section.
/^### / && section == ops_section {
  close_rule()
  op = substr(line, 5)
  sub(/ .*$/, "", op)
  gsub(/`/, "", op)
  if (op != "Views") {
    if (op in op_line) problem("operation " op " has two headings")
    op_line[op] = NR
  }
  next
}

# Rule headings.
/^#### / {
  close_rule()
  rest = substr(line, 6)
  if (match(rest, "^" tok_re)) {
    title = substr(rest, RLENGTH + 2)
    found = ids_in(substr(rest, RSTART, RLENGTH), NR)
    sub(/^ /, "", found)
    if (found != "") {
      if (found in defined) problem(found " is defined twice (lines " defined[found] " and " NR ")")
      else defined[found] = NR
      area_key = found
      sub(/-[0-9]+$/, "", area_key)
      seq = found
      sub(/^.*-/, "", seq)
      if ((area_key in area_last) && seq + 0 < area_last[area_key] + 0) {
        problem(found " at line " NR " comes after " area_key "-" area_last_txt[area_key] "; keep each area in numeric order")
      }
      if (!(area_key in area_last) || seq + 0 > area_last[area_key] + 0) {
        area_last[area_key] = seq
        area_last_txt[area_key] = seq
      }
      current = found
      if (title ~ /^Retired:/) retired[found] = NR
      field = ""
      fields_started = 0
      pending_blank = 0
      has_pre = has_eff = has_err = has_off = 0
    }
  }
  next
}

/^#+ / { close_rule() }

current != "" && /^- (Precondition|Effect|Error|Off chain):/ { field = ""; fields_started = 1 }
current != "" && /^[ \t]*$/ { field = "" }
current != "" && /^- Precondition:/ { has_pre = 1; field = "pre" }
current != "" && /^- Effect:/ { has_eff = 1 }
current != "" && /^- Off chain:/ { has_off = 1 }
# The rules a precondition includes, on its own line or its indented sub-bullets.
current != "" && field == "pre" {
  found = ids_in(line, NR)
  k = split(found, got, " ")
  for (j = 1; j <= k; j++) if (got[j] != current) pre_refs[current] = pre_refs[current] " " got[j]
}
current != "" && /^- Error:/ {
  has_err = 1
  text = substr(line, 10)
  sub(/^ +/, "", text)
  sub(/ +$/, "", text)
  if (text == "") problem(current " has an empty \"- Error:\" field; write none or the error names")
  err_text[current] = text
  rest = line
  while (match(rest, /`[A-Z][A-Z0-9_]*`/)) {
    name = substr(rest, RSTART + 1, RLENGTH - 2)
    cited_error[name] = cited_error[name] " " current
    rest = substr(rest, RSTART + RLENGTH)
  }
}

# Error table rows: | 10 | `UNAUTHORIZED` | ...
section == "Errors" && /^\| *[0-9]+ *\| *`[A-Z][A-Z0-9_]*`/ {
  code = line
  sub(/^\| */, "", code)
  sub(/ *\|.*$/, "", code)
  if (code in error_code) problem("error code " code " is listed twice (lines " error_code[code] " and " NR ")")
  else error_code[code] = NR
  if (match(line, /`[A-Z][A-Z0-9_]*`/)) {
    name = substr(line, RSTART + 1, RLENGTH - 2)
    if (name in known_error) problem("error name " name " is listed twice")
    known_error[name] = 1
  }
}

# Rule index rows: | `MANDATE-ROLE-001` | ...
section == "Rule index" && /^\| *`/ {
  if (match(line, "^\\| *`" tok_re "`")) {
    found = ids_in(substr(line, RSTART, RLENGTH), NR)
    sub(/^ /, "", found)
    if (found != "") {
      if (found in indexed) problem(found " is listed twice in the rule index")
      indexed[found] = NR
    }
  }
  next
}

# Check order blocks.
/^<!-- check-order: [a-z_]+ -->$/ {
  if (in_order) order_problem("check order block at line " NR " opens inside another block")
  in_order = 1
  order_name = line
  sub(/^<!-- check-order: /, "", order_name)
  sub(/ -->$/, "", order_name)
  if (order_name in order_seen) order_problem("check order " order_name " appears twice")
  order_seen[order_name] = NR
  if (section != ops_section || op == "") order_problem("check order " order_name " at line " NR " is not under an operation heading")
  else if (op != order_name) order_problem("check order " order_name " at line " NR " sits under the heading for " op)
  order_lines = 0
  prev_n = 0
  prev_letter = ""
  next
}
/^<!-- \/check-order -->$/ {
  if (!in_order) order_problem("check order block closed at line " NR " without being opened")
  else if (order_lines == 0) order_problem("check order " order_name " has no numbered checks")
  else { block_max[order_name] = prev_n; last_block = order_name }
  in_order = 0
  next
}
in_order && /^[ \t]*$/ { next }
in_order {
  if (line !~ /^[0-9]+[a-z]?\. /) {
    order_problem(order_name ": line " NR " is not a numbered check (\"1. \" or \"9a. \")")
    next
  }
  order_lines++
  numtxt = line
  sub(/\..*$/, "", numtxt)
  letter = numtxt
  sub(/^[0-9]+/, "", letter)
  n = numtxt
  sub(/[a-z]$/, "", n)
  n = n + 0
  if (letter == "") {
    if (n != prev_n + 1) order_problem(order_name ": line " NR " is step " n ", expected step " (prev_n + 1))
    prev_letter = ""
  } else if (n != prev_n || letter != substr("abcdefghijklmnopqrstuvwxyz", index("abcdefghijklmnopqrstuvwxyz", (prev_letter == "" ? "`" : prev_letter)) + 1, 1)) {
    order_problem(order_name ": line " NR " is step " numtxt ", which does not follow step " prev_n prev_letter)
  } else {
    prev_letter = letter
  }
  if (letter == "") prev_n = n
  found = ids_in(line, NR)
  k = split(found, got, " ")
  m = 0
  first = ""
  for (j = 1; j <= k; j++) if (got[j] ~ /^MANDATE-/) { m++; if (first == "") first = got[j] }
  if (m != 1) order_problem(order_name ": line " NR " cites " m " rule IDs, needs exactly one")
  else {
    order_ref[NR] = first
    order_block[NR] = order_name
    order_step[NR] = numtxt
    block_rules[order_name] = block_rules[order_name] " " first "@" numtxt
  }
}

# Check ranges: "`op` checks N to M" names an order, and a bare "Checks N to
# M" means the order closed above it under the same heading. The phrasings
# "check", "checks", "step" or "steps" followed by "N to M", "N through M",
# "N-M" or "N and M" are all read. Every range is checked at the end, once
# every block is known.
/^#+ / { last_block = "" }
{
  rest = line
  while (match(rest, /(`[^`]+` )?([Cc]hecks?|[Ss]teps?) [0-9]+( to | through |-| and )[0-9]+/)) {
    frag = substr(rest, RSTART, RLENGTH)
    rest = substr(rest, RSTART + RLENGTH)
    named = (frag ~ /^`/)
    rname = ""
    phrase = frag
    if (named) { rname = frag; sub(/^`/, "", rname); sub(/`.*$/, "", rname); sub(/^`[^`]+` /, "", phrase) }
    else rname = last_block
    nums = phrase
    sub(/^[A-Za-z]+ /, "", nums)
    match(nums, /^[0-9]+/)
    first_n = substr(nums, RSTART, RLENGTH) + 0
    last_txt = nums
    sub(/^.*[^0-9]/, "", last_txt)
    nrange++
    range_name[nrange] = rname
    range_named[nrange] = named
    range_phrase[nrange] = phrase
    range_from[nrange] = first_n
    range_to[nrange] = last_txt + 0
    range_and[nrange] = (phrase ~ / and /)
    range_line[nrange] = NR
  }
}

# Every ID cited anywhere, outside headings and the index.
{
  found = ids_in(line, NR)
  k = split(found, got, " ")
  for (j = 1; j <= k; j++) {
    if (!(got[j] in cited)) cited[got[j]] = NR
    if (got[j] != current && !(got[j] in cited_elsewhere)) cited_elsewhere[got[j]] = NR
  }
}

# Check order problems count only with --check-order.
function order_problem(msg) {
  if (check_order) problem(msg)
}

END {
  close_rule()
  if (in_order) order_problem("check order " order_name " is never closed")

  for (id in defined) if (!(id in indexed)) problem(id " is defined at line " defined[id] " but missing from the rule index")
  for (id in indexed) if (!(id in defined)) problem(id " is in the rule index but not defined")
  for (id in cited) if (!(id in defined)) problem(id " is cited at line " cited[id] " but not defined")
  for (i = 1; i <= nrange; i++) {
    rn = range_name[i]
    where = "line " range_line[i] " cites " rn " checks " range_from[i] " to " range_to[i]
    if (!range_named[i] && rn == "") order_problem("line " range_line[i] " has a bare check range (" range_phrase[i] ") with no check order above it in its section; name the order")
    else if (!(rn in order_seen)) order_problem(where ", but there is no " rn " check order")
    else if (range_from[i] < 1 || (!range_and[i] && range_from[i] > range_to[i]) || range_to[i] > block_max[rn] + 0 || range_from[i] > block_max[rn] + 0) order_problem(where ", but " rn " has " block_max[rn] + 0 " checks")
  }
  nl = split(retired_list, rl, " ")
  for (i = 1; i <= nl; i++) {
    listed[rl[i]] = 1
    if (!(rl[i] in defined)) problem(rl[i] " is listed in retired-ids.txt but not defined; keep retired rules, never delete them")
    else if (!(rl[i] in retired)) problem(rl[i] " is listed in retired-ids.txt but its title does not start with Retired:")
  }
  for (id in retired) if (!(id in listed)) problem(id " is retired but not listed in retired-ids.txt")
  for (id in retired) {
    if (err_text[id] != "none") problem(id " is retired, so its Error line must be none")
    if (id in cited_elsewhere) problem(id " is retired but cited at line " cited_elsewhere[id] "; cite the rule that replaced it")
  }

  for (name in cited_error) if (!(name in known_error)) problem("error " name " is cited by" cited_error[name] " but not in the error table")

  split(mandate_areas, ma, " ")
  for (i = 1; i in ma; i++) count_m[ma[i]] = 0
  split(chainops_areas, ca, " ")
  for (i = 1; i in ca; i++) count_c[ca[i]] = 0
  for (id in defined) {
    split(id, parts, "-")
    if (parts[1] == "MANDATE") {
      if (!(parts[2] in count_m)) problem(id " uses an area that is not in the area list")
      else count_m[parts[2]]++
    } else {
      if (!(parts[2] in count_c)) problem(id " uses an area that is not in the area list")
      else count_c[parts[2]]++
    }
  }
  for (i = 1; i in ma; i++) {
    a = ma[i]
    if (count_m[a] == 0) problem("rule area MANDATE-" a " has no rule")
    else {
      errs = ""
      for (name in cited_error) if (index(cited_error[name], " MANDATE-" a "-") > 0) errs = errs " " name
      printf "area MANDATE-%s: %d rule(s); errors:%s\n", a, count_m[a], (errs == "" ? " none" : errs)
    }
  }
  for (i = 1; i in ca; i++) {
    a = ca[i]
    if (count_c[a] == 0) problem("rule area CHAINOPS-" a " has no rule")
    else printf "area CHAINOPS-%s: %d rule(s), off chain, excluded from vector coverage\n", a, count_c[a]
  }

  if (check_order) {
    blocks = 0
    for (b in order_seen) blocks++
    if (blocks == 0) problem("no check order blocks found")
    ops = 0
    for (o in op_line) {
      ops++
      if (!(o in order_seen)) problem("operation " o " at line " op_line[o] " has no check order block")
    }
    for (ln in order_ref) {
      id = order_ref[ln]
      if (!(id in defined)) problem("check order line " ln " cites " id ", which is not defined")
      else if (err_text[id] ~ /^none/) problem("check order line " ln " cites " id ", which returns no error")
    }
    # A step may not cite a rule whose precondition includes, directly or
    # through rules that no step of the same order checks, a rule that another
    # step checks: the first error would depend on how an implementation read
    # it. Visited rules are tracked, so reference cycles end.
    for (ln in order_ref) {
      a = order_ref[ln]
      blk = order_block[ln]
      delete step_of
      nb = split(block_rules[blk], steps, " ")
      for (y = 1; y <= nb; y++) {
        split(steps[y], sp, "@")
        if (!(sp[1] in step_of)) step_of[sp[1]] = sp[2]
      }
      delete seen
      delete via
      top = 0
      seen[a] = 1
      nr = split(pre_refs[a], inc, " ")
      for (x = 1; x <= nr; x++) { top++; stack[top] = inc[x]; via[inc[x]] = "" }
      while (top > 0) {
        b = stack[top]
        top--
        if (b in seen) continue
        seen[b] = 1
        if ((b in step_of) && step_of[b] != order_step[ln]) {
          problem(blk ": line " ln " cites " a ", whose precondition includes " b (via[b] == "" ? "" : " (through " via[b] ")") ", which step " step_of[b] " also checks")
          continue
        }
        if (b in step_of) continue
        nr2 = split(pre_refs[b], inc2, " ")
        for (x = 1; x <= nr2; x++) {
          if (!(inc2[x] in seen)) {
            top++
            stack[top] = inc2[x]
            if (!(inc2[x] in via)) via[inc2[x]] = (via[b] == "" ? b : via[b] ", " b)
          }
        }
      }
    }
    if (problems == 0) printf "check orders: %d block(s) for %d operation(s), every numbered check cites one defined rule\n", blocks, ops
  }

  total = 0
  for (id in defined) total++
  if (problems > 0) {
    printf "check-spec-ids: %d problem(s)\n", problems > "/dev/stderr"
    exit 1
  }
  printf "%d rule ID(s) defined, indexed and unique\n", total
}
' "$spec"
awk_status=$?
set -e
if [ "$awk_status" -ne 0 ]; then exit "$awk_status"; fi
if [ "$charset_status" -ne 0 ]; then exit "$charset_status"; fi
exit "$retired_status"
