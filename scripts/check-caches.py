#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# A restored cache is trusted code: what comes back in soroban/target is linked
# into the next build without being compiled again, so a cache written by another
# branch must never seed one. This check reads every workflow and fails when:
#
# - a cache step that covers a build target directory has restore-keys, because a
#   fallback key is what crosses branches;
# - that step's key is not exact, meaning it does not name the ref, the lockfile
#   and the toolchain file;
# - that step's path cannot be read here because it comes from an expression;
# - a workflow uses an action that caches a target directory on its own terms,
#   such as rust-cache, whose keys this check cannot see;
# - a workflow whose file name says release or deploy uses any cache at all,
#   including the cache input of a setup action. What ships is built clean.
#
# Workflows are read line by line, by indentation, with no YAML dependency. A step
# is the block from its "- " line to the next line indented no deeper.
#
# Standard library only.
#
# Usage: scripts/check-caches.py [workflow file or directory]...
#   With no argument, reads .github/workflows in this repository.
# Exit codes: 0 clean, 1 findings named on stderr, 2 nothing to read or a file
# that cannot be read.

import os
import re
import sys

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
DEFAULT = os.path.join(ROOT, ".github", "workflows")

CACHE_ACTION = re.compile(r"^\s*(?:-\s+)?uses:\s*['\"]?actions/cache(?:/restore|/save)?@")
SELF_KEYED_ACTION = re.compile(r"^\s*(?:-\s+)?uses:\s*['\"]?[^'\"\s]*rust-cache[^'\"\s]*@", re.IGNORECASE)
SETUP_CACHE_INPUT = re.compile(r"^\s*(cache|cache-[a-z-]+):\s*(\S+)")
USES = re.compile(r"^\s*(?:-\s+)?uses:\s*['\"]?([^'\"\s#]+)")
TARGET_PATH = re.compile(r"(?:^|/)target(?:/|$)")
SHIPPING_NAME = re.compile(r"release|deploy", re.IGNORECASE)
# What an exact key must name, each with what it pins.
KEY_PARTS = (
    ("github.ref", "the ref, so a cache never crosses branches"),
    ("Cargo.lock", "the lockfile"),
    ("rust-toolchain.toml", "the toolchain file"),
)


def indent_of(line):
    return len(line) - len(line.lstrip(" "))


def steps_of(lines):
    """Yields (first line index, lines) for every "- " block that holds a uses:."""
    for index, line in enumerate(lines):
        if not USES.match(line):
            continue
        start = index
        if not line.lstrip().startswith("- "):
            while start > 0 and not lines[start].lstrip().startswith("- "):
                start -= 1
        dash = indent_of(lines[start])
        end = start + 1
        while end < len(lines) and (not lines[end].strip() or indent_of(lines[end]) > dash):
            end += 1
        yield index, lines[start:end]


def input_values(step, name):
    """The lines of one input under with:, its own line first, or None."""
    for offset, line in enumerate(step):
        match = re.match(r"^(\s*)%s:\s*(.*)$" % re.escape(name), line)
        if not match:
            continue
        depth = len(match.group(1))
        values = [match.group(2)] if match.group(2) and match.group(2) not in ("|", ">", "|-", ">-") else []
        for later in step[offset + 1 :]:
            if later.strip() and indent_of(later) <= depth:
                break
            if later.strip():
                values.append(later.strip())
        return values
    return None


def check_cache_step(label, line_number, step):
    problems = []
    paths = input_values(step, "path") or []
    if any("${{" in path for path in paths):
        problems.append("%s:%d: the cache path is an expression, so what it covers cannot be checked" % (label, line_number))
        return problems
    if not any(TARGET_PATH.search(path.strip().strip("'\"")) for path in paths):
        return problems
    target = next(p for p in paths if TARGET_PATH.search(p.strip().strip("'\"")))
    if input_values(step, "restore-keys") is not None:
        problems.append("%s:%d: restore-keys for %s; a target directory is restored from an exact key only" % (label, line_number, target))
    key = " ".join(input_values(step, "key") or [])
    if not key:
        problems.append("%s:%d: no key for %s" % (label, line_number, target))
    else:
        for part, why in KEY_PARTS:
            if part not in key:
                problems.append("%s:%d: the key for %s does not name %s (%s)" % (label, line_number, target, part, why))
    return problems


def check_file(path, label):
    with open(path, encoding="utf-8") as handle:
        lines = handle.read().split("\n")
    shipping = bool(SHIPPING_NAME.search(os.path.basename(path)))
    problems = []
    for index, step in steps_of(lines):
        number = index + 1
        action = USES.match(lines[index]).group(1)
        if CACHE_ACTION.match(lines[index]):
            if shipping:
                problems.append("%s:%d: %s in a release or deploy workflow, which must build with no cache" % (label, number, action))
            problems.extend(check_cache_step(label, number, step))
        elif SELF_KEYED_ACTION.match(lines[index]):
            problems.append("%s:%d: %s keeps its own keys for the target directory, use actions/cache with an exact key" % (label, number, action))
        elif shipping:
            for line in step:
                match = SETUP_CACHE_INPUT.match(line)
                if match and match.group(2).strip("'\"").lower() not in ("false", "''", ""):
                    problems.append("%s:%d: %s has a cache input in a release or deploy workflow, which must build with no cache" % (label, number, action))
    return problems


def workflow_files(argument):
    if os.path.isdir(argument):
        return sorted(
            os.path.join(argument, name)
            for name in os.listdir(argument)
            if name.endswith((".yml", ".yaml"))
        )
    return [argument]


def main(argv):
    arguments = argv[1:] or [DEFAULT]
    files = [path for argument in arguments for path in workflow_files(argument)]
    if not files:
        print("check-caches: no workflow files in %s" % " ".join(arguments), file=sys.stderr)
        return 2
    problems = []
    for path in files:
        label = os.path.relpath(path, ROOT) if os.path.abspath(path).startswith(ROOT + os.sep) else path
        try:
            problems.extend(check_file(path, label))
        except (OSError, UnicodeDecodeError) as err:
            print("check-caches: cannot read %s: %s" % (label, err), file=sys.stderr)
            return 2
    if problems:
        print("check-caches: %d problem(s):" % len(problems), file=sys.stderr)
        for problem in problems:
            print("  " + problem, file=sys.stderr)
        return 1
    print("check-caches: %d workflow file(s) read, every target cache has an exact key" % len(files))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
