#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Checks that the Rust error enum matches spec/errors.json entry for entry.
# A contract returns only the number and the spec gives it its meaning, so a
# single code out of step would make the two disagree without anything
# failing.
#
# The Rust side is the one #[contracterror] enum in
# soroban/crates/mandate-errors/src/lib.rs. Comments are ignored; string and
# character literals are kept whole, so a // inside one is not taken for a
# comment. Every variant must read `Name = <decimal code>,` and anything else
# in the enum body, such as an attribute or a variant without a code, is
# reported, so a variant cannot get past the check in a form it does not read.
# The enum must be #[repr(u32)], the width of a contract error code.
#
# Names are compared in upper snake case on both sides: AgentRemoved is read
# as AGENT_REMOVED. An acronym such as PayoutIDExists reads as PAYOUT_IDEXISTS
# and matches nothing.
#
# Every difference is named, all in one run:
#   missing in rust   a table entry with no variant of that name or code
#   extra in rust     a variant with no table entry of that name or code
#   renumbered code   a name with one code in the table and another in Rust
#   renamed entry     a code with one name in the table and another in Rust
# A name or code listed twice on either side is reported as well.
#
# Standard library only, no dependencies.
#
# Usage: scripts/check-error-codes.py [--list] [--spec FILE] [--rust FILE]
#   --list  also print every matched code with its table and Rust names
# Exit codes: 0 the two agree, 1 differences named on stderr, 2 bad usage or a
# file that cannot be read or parsed.

import argparse
import json
import os
import re
import sys

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
SPEC = os.path.join(ROOT, "spec", "errors.json")
RUST = os.path.join(ROOT, "soroban", "crates", "mandate-errors", "src", "lib.rs")

U32_MAX = 2**32 - 1
UPPER_SNAKE = re.compile(r"^[A-Z][A-Z0-9_]*$")
UPPER_CAMEL = re.compile(r"^[A-Z][A-Za-z0-9]*$")
VARIANT = re.compile(r"([A-Za-z_][A-Za-z0-9_]*)\s*=\s*([0-9]+)")
ATTRIBUTE = re.compile(r"#\s*\[\s*(?:soroban_sdk\s*::\s*)?contracterror\b")
ENUM_HEAD = re.compile(r"(?:pub(?:\s*\([^)]*\))?\s+)?enum\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{")
CHAR_LITERAL = re.compile(r"'(?:[^'\\\n]|\\(?:[nrt0\\'\"]|x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]{1,6}\}))'")


class Unreadable(Exception):
    """A file the check cannot read or parse."""


def shown(path):
    """The path relative to the repository when it is inside it."""
    rel = os.path.relpath(os.path.abspath(path), ROOT)
    return path if rel == os.pardir or rel.startswith(os.pardir + os.sep) else rel


def upper_snake(name):
    """AgentRemoved becomes AGENT_REMOVED."""
    return re.sub(r"(?<=[a-z0-9])(?=[A-Z])", "_", name).upper()


def upper_camel(name):
    """AGENT_REMOVED becomes AgentRemoved, the variant to add for it."""
    return "".join(word[:1] + word[1:].lower() for word in name.split("_"))


def read_text(path):
    try:
        with open(path, encoding="utf-8") as handle:
            return handle.read()
    except (OSError, UnicodeDecodeError) as err:
        raise Unreadable("cannot read %s: %s" % (shown(path), err))


def no_duplicate_keys(pairs):
    keys = [key for key, _ in pairs]
    for key in keys:
        if keys.count(key) > 1:
            raise ValueError("the key %s appears twice in one object" % key)
    return dict(pairs)


def read_spec(path):
    """Returns [(name, code)] in table order."""
    label = shown(path)
    try:
        data = json.loads(read_text(path), object_pairs_hook=no_duplicate_keys)
    except ValueError as err:
        raise Unreadable("%s is not valid JSON: %s" % (label, err))
    if not isinstance(data, list):
        raise Unreadable("%s must be a list of { name, code, summary }" % label)
    entries = []
    for number, entry in enumerate(data, 1):
        name = entry.get("name") if isinstance(entry, dict) else None
        code = entry.get("code") if isinstance(entry, dict) else None
        if not isinstance(name, str) or not UPPER_SNAKE.match(name):
            raise Unreadable("%s entry %d has no upper snake case name" % (label, number))
        if not isinstance(code, int) or isinstance(code, bool) or not 0 <= code <= U32_MAX:
            raise Unreadable("%s entry %d (%s) has a code that is not a u32" % (label, number, name))
        entries.append((name, code))
    return entries


def strip_comments(text):
    """Blanks out comments but keeps their newlines, so line numbers hold."""
    out, i, n, depth = [], 0, len(text), 0
    while i < n:
        pair = text[i:i + 2]
        if depth:
            if pair in ("/*", "*/"):
                depth += 1 if pair == "/*" else -1
                out.append("  ")
                i += 2
            else:
                out.append("\n" if text[i] == "\n" else " ")
                i += 1
        elif pair == "/*":
            depth = 1
            out.append("  ")
            i += 2
        elif pair == "//":
            end = text.find("\n", i)
            end = n if end < 0 else end
            out.append(" " * (end - i))
            i = end
        elif text[i] == '"':
            end = i + 1
            while end < n and text[end] != '"':
                end += 2 if text[end] == "\\" else 1
            if end >= n:
                raise Unreadable("a string literal never ends")
            out.append(text[i:end + 1])
            i = end + 1
        else:
            literal = CHAR_LITERAL.match(text, i) if text[i] == "'" else None
            piece = literal.group(0) if literal else text[i]
            out.append(piece)
            i += len(piece)
    if depth:
        raise Unreadable("a block comment never ends")
    return "".join(out)


def attribute_end(text, pos):
    """The index just past the attribute whose # is at pos."""
    start = pos + 1
    while start < len(text) and text[start].isspace():
        start += 1
    if not text.startswith("[", start):
        raise Unreadable("a # that does not start an attribute")
    depth = 0
    for i in range(start, len(text)):
        if text[i] == "[":
            depth += 1
        elif text[i] == "]":
            depth -= 1
            if depth == 0:
                return i + 1
    raise Unreadable("an attribute never closes")


def read_rust(path):
    """Returns (enum name, is repr(u32), [(name, code, line)], [problem])."""
    label = shown(path)
    try:
        text = strip_comments(read_text(path))
    except Unreadable as err:
        raise Unreadable("%s: %s" % (label, err))
    found = list(ATTRIBUTE.finditer(text))
    if len(found) != 1:
        if not found:
            raise Unreadable("no #[contracterror] enum in %s" % label)
        raise Unreadable("%d #[contracterror] enums in %s; the check reads exactly one" % (len(found), label))

    pos, attributes = found[0].start(), []
    while True:
        while pos < len(text) and text[pos].isspace():
            pos += 1
        if not text.startswith("#", pos):
            break
        try:
            end = attribute_end(text, pos)
        except Unreadable as err:
            raise Unreadable("%s: cannot read the attributes on the #[contracterror] enum: %s" % (label, err))
        attributes.append(re.sub(r"\s+", "", text[pos:end]))
        pos = end
    head = ENUM_HEAD.match(text, pos)
    if not head:
        raise Unreadable("%s: the #[contracterror] attribute is not on an enum" % label)
    body_start = head.end()
    body_end = text.find("}", body_start)
    if body_end < 0 or "{" in text[body_start:body_end]:
        raise Unreadable("%s: cannot find the end of enum %s" % (label, head.group(1)))

    variants, problems, offset = [], [], body_start
    for piece in text[body_start:body_end].split(","):
        item = piece.strip()
        line = text.count("\n", 0, offset + len(piece) - len(piece.lstrip())) + 1
        offset += len(piece) + 1
        if not item:
            continue
        match = VARIANT.fullmatch(item)
        if not match or not UPPER_CAMEL.match(match.group(1)) or int(match.group(2)) > U32_MAX:
            problems.append(
                "cannot read '%s' at %s:%d; write each variant as Name = <decimal code>"
                % (" ".join(item.split()), label, line)
            )
            continue
        variants.append((match.group(1), int(match.group(2)), line))
    return head.group(1), "#[repr(u32)]" in attributes, variants, problems


def compare(spec, rust, spec_label, rust_label):
    """Returns ([(code, spec name, rust name)] matched, [problem])."""
    found = []  # (code, message), sorted by code at the end
    spec_names, spec_codes = {}, {}
    for name, code in spec:
        if name in spec_names:
            found.append((code, "%s lists %s twice" % (spec_label, name)))
        elif code in spec_codes:
            found.append((code, "%s lists code %d twice, for %s and %s" % (spec_label, code, spec_codes[code], name)))
        else:
            spec_names[name], spec_codes[code] = code, name
    rust_names, rust_codes = {}, {}
    for variant, code, line in rust:
        name, where = upper_snake(variant), "%s:%d" % (rust_label, line)
        if name in rust_names:
            found.append((code, "%s lists %s twice, at %s" % (rust_label, variant, where)))
        elif code in rust_codes:
            found.append((code, "%s lists code %d twice, for %s and %s at %s" % (rust_label, code, rust_codes[code], variant, where)))
        else:
            rust_names[name], rust_codes[code] = (code, variant, where), variant

    matched, unmatched_spec = [], []
    for name, code in spec_names.items():
        if name not in rust_names:
            unmatched_spec.append((name, code))
            continue
        rust_code, variant, where = rust_names[name]
        if rust_code == code:
            matched.append((code, name, variant))
        else:
            found.append((code, "renumbered code: %s is %d in %s but %d in %s" % (name, code, spec_label, rust_code, where)))
    unmatched_rust = {
        code: (name, variant, where)
        for name, (code, variant, where) in rust_names.items()
        if name not in spec_names
    }
    for name, code in unmatched_spec:
        if code in unmatched_rust:
            rust_name, variant, where = unmatched_rust.pop(code)
            found.append((code, "renamed entry: code %d is %s in %s but %s (%s) in %s"
                          % (code, name, spec_label, rust_name, variant, where)))
        else:
            found.append((code, "missing in rust: %s (%d) is in %s but not in %s; add %s = %d"
                          % (name, code, spec_label, rust_label, upper_camel(name), code)))
    for code, (name, variant, where) in unmatched_rust.items():
        found.append((code, "extra in rust: %s (%d), %s at %s, is not in %s" % (name, code, variant, where, spec_label)))
    return sorted(matched), [message for _, message in sorted(found, key=lambda item: item[0])]


def main(argv):
    parser = argparse.ArgumentParser(
        prog="check-error-codes.py",
        description="Checks that the Rust error enum matches spec/errors.json entry for entry.",
    )
    parser.add_argument("--list", action="store_true", help="also print every matched code with its names")
    parser.add_argument("--spec", default=SPEC, help="the error table (default: spec/errors.json)")
    parser.add_argument("--rust", default=RUST, help="the Rust file with the enum (default: the mandate-errors crate)")
    args = parser.parse_args(argv[1:])

    spec_label, rust_label = shown(args.spec), shown(args.rust)
    try:
        spec = read_spec(args.spec)
        enum, is_u32, rust, problems = read_rust(args.rust)
    except Unreadable as err:
        print("check-error-codes: %s" % err, file=sys.stderr)
        return 2

    if not is_u32:
        problems.append(
            "enum %s in %s is not #[repr(u32)] (the attributes after #[contracterror] are read); "
            "a contract error code is a u32" % (enum, rust_label)
        )
    matched, differences = compare(spec, rust, spec_label, rust_label)
    problems.extend(differences)

    if args.list:
        for code, name, variant in matched:
            print("  %d %s %s" % (code, name, variant))
    if problems:
        print("check-error-codes: %d difference(s) between %s and enum %s in %s:"
              % (len(problems), spec_label, enum, rust_label), file=sys.stderr)
        for problem in problems:
            print("  " + problem, file=sys.stderr)
        return 1
    print("check-error-codes: the %d code(s) in %s match enum %s in %s" % (len(matched), spec_label, enum, rust_label))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
