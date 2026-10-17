#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Fails on literal invisible, bidirectional control and separator characters
# in tracked text files. Literal bidirectional controls can make code read
# differently from how it runs, and none of these characters show in review.
# Tests and data that need one must write it as an escape.
#
# Rejected in every file: every character in the Unicode categories Cf
# (format, including U+FEFF, U+200B to U+200F, U+202A to U+202E and U+2066 to
# U+2069), Zl (U+2028) and Zp (U+2029). Categories come from Python's
# unicodedata tables, not a hand written list.
#
# Rejected unless the file is listed in scripts/invisible-characters-allowlist.txt:
# the lookalike no break space (U+00A0), narrow no break space (U+202F) and
# minus sign (U+2212).
#
# Files are read as bytes from the working tree. Content with a NUL byte is
# binary and skipped, whatever its extension; anything else must be valid
# UTF-8. Symlinks and submodules are skipped. A tracked file missing from the
# working tree fails.
#
# Standard library only, no dependencies.
#
# Usage: scripts/check-invisible-characters.py [path inside the repo]
# Exit codes: 0 clean, 1 findings named on stderr, 2 the tree could not be
# listed.

import os
import stat
import subprocess
import sys
import unicodedata

ALLOWLIST = "scripts/invisible-characters-allowlist.txt"
ALWAYS_REJECTED = {"Cf", "Zl", "Zp"}
LOOKALIKES = {0x00A0, 0x202F, 0x2212}


def scan(data, allow_lookalikes):
    """Returns ("binary", None), ("invalid", None) or ("text", findings).

    Each finding is (line, column, code point), with the column counted in
    code points from 1. A byte order mark is kept and reported.
    """
    if b"\0" in data:
        return "binary", None
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        return "invalid", None
    findings = []
    for line_number, line in enumerate(text.split("\n"), start=1):
        for column, character in enumerate(line, start=1):
            point = ord(character)
            if unicodedata.category(character) in ALWAYS_REJECTED or (
                point in LOOKALIKES and not allow_lookalikes
            ):
                findings.append((line_number, column, point))
    return "text", findings


def parse_allowlist(text):
    paths = set()
    for raw in text.split("\n"):
        path = raw.split("#", 1)[0].strip()
        if path:
            paths.add(path)
    return paths


def show_path(name):
    """A file name safe to print in a log, escaped when it holds anything
    other than printable text."""
    try:
        text = name.decode("utf-8")
        unsafe = any(
            ord(c) < 0x20
            or 0x7F <= ord(c) <= 0x9F
            or unicodedata.category(c) in ALWAYS_REJECTED
            or c in "\"\\"
            for c in text
        )
        if not unsafe:
            return text
    except UnicodeDecodeError:
        pass
    escaped = "".join(
        chr(b) if 0x20 <= b < 0x7F and b not in (0x22, 0x5C) else "\\x%02x" % b for b in name
    )
    return '"%s"' % escaped


def code_point(point):
    return "U+%04X" % point


def git(args, cwd):
    # An inherited repo override would make git look at some other repo.
    env = {k: v for k, v in os.environ.items() if k not in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE")}
    return subprocess.run(["git", "-C", cwd, *args], env=env, capture_output=True, check=False)


def main(argv):
    start = argv[1] if len(argv) > 1 else os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
    top = git(["rev-parse", "--show-toplevel"], start)
    if top.returncode != 0:
        print("check-invisible-characters: %s is not inside a git work tree" % start, file=sys.stderr)
        return 2
    root = os.fsencode(top.stdout.decode("utf-8", "surrogateescape").rstrip("\n"))
    listing = git(["ls-files", "-z", "--cached"], root)
    if listing.returncode != 0:
        print("check-invisible-characters: git ls-files failed in %s" % os.fsdecode(root), file=sys.stderr)
        return 2

    allowlist_path = os.path.join(root, os.fsencode(ALLOWLIST))
    allowed = set()
    if os.path.isfile(allowlist_path):
        with open(allowlist_path, "rb") as handle:
            allowed = parse_allowlist(handle.read().decode("utf-8"))

    problems = []
    text_files = 0
    binary_files = 0
    seen = set()
    for name in listing.stdout.split(b"\0"):
        # git lists a conflicted file once per stage; check it once.
        if not name or name in seen:
            continue
        seen.add(name)
        shown = show_path(name)
        path = os.path.join(root, name)
        try:
            mode = os.lstat(path).st_mode
        except FileNotFoundError:
            problems.append("%s: tracked but missing from the working tree" % shown)
            continue
        if not stat.S_ISREG(mode):
            continue
        with open(path, "rb") as handle:
            data = handle.read()
        kind, findings = scan(data, name.decode("utf-8", "replace") in allowed)
        if kind == "binary":
            binary_files += 1
        elif kind == "invalid":
            problems.append("%s: not valid UTF-8 text" % shown)
        else:
            text_files += 1
            for line, column, point in findings:
                problems.append("%s:%d:%d: %s" % (shown, line, column, code_point(point)))

    version = unicodedata.unidata_version
    if problems:
        print("check-invisible-characters: failed (Unicode %s):" % version, file=sys.stderr)
        for problem in problems:
            print("  " + problem, file=sys.stderr)
        print(
            "Write these characters as escapes. Lookalike spaces and minus signs may be "
            "allowed per file in %s." % ALLOWLIST,
            file=sys.stderr,
        )
        return 1
    print(
        "check-invisible-characters: %d text file(s) checked, %d binary file(s) skipped (Unicode %s)"
        % (text_files, binary_files, version)
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
