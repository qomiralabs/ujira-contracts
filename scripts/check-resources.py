#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Resource usage decides whether a batch fits in one transaction and what each
# payout costs, so a change that quietly adds a storage write per item must show
# in the pull request that causes it. Compares a fresh resource snapshot with the
# committed soroban/resource-snapshot.json, metric by metric, within the margin in
# ci/resource-budget.json:
#
# - a metric above the margin fails, naming the scenario and the metric;
# - a metric below the margin fails too, so the snapshot stays current and a later
#   regression cannot hide inside an old improvement;
# - a scenario on only one side fails.
#
# The fresh run comes from the resource-snapshot binary in soroban/vector-runner,
# against the wasm scripts/soroban-build.sh leaves, unless --fresh names a file.
# All arithmetic is on whole numbers.
#
# Standard library only.
#
# Usage: scripts/check-resources.py [--fresh FILE] [--snapshot FILE] [--budget FILE]
#        scripts/check-resources.py --update
#   --update  run the harness and write the committed snapshot, for a pull request
#             that changes usage on purpose
# Exit codes: 0 within the margin, 1 differences named on stderr, 2 bad usage or a
# file or run that cannot be read.

import argparse
import json
import os
import subprocess
import sys

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
SNAPSHOT = os.path.join(ROOT, "soroban", "resource-snapshot.json")
BUDGET = os.path.join(ROOT, "ci", "resource-budget.json")
METRICS = ("instructions", "memory_bytes", "read_entries", "write_entries", "read_bytes", "write_bytes")
UPDATE_HINT = "run scripts/check-resources.py --update and commit the snapshot if the change is intended"


class Unreadable(Exception):
    """A file or a run the check cannot use."""


def no_duplicate_keys(pairs):
    keys = [key for key, _ in pairs]
    for key in keys:
        if keys.count(key) > 1:
            raise ValueError("the key %s appears twice in one object" % key)
    return dict(pairs)


def parse_snapshot(text, label):
    try:
        data = json.loads(text, object_pairs_hook=no_duplicate_keys)
    except ValueError as err:
        raise Unreadable("%s is not valid JSON: %s" % (label, err))
    scenarios = data.get("scenarios") if isinstance(data, dict) else None
    if not isinstance(scenarios, dict) or not scenarios:
        raise Unreadable("%s has no scenarios" % label)
    for name, entry in scenarios.items():
        if not isinstance(entry, dict) or set(entry) != set(METRICS):
            raise Unreadable("%s: scenario %s must have exactly %s" % (label, name, ", ".join(METRICS)))
        for metric, value in entry.items():
            if not isinstance(value, int) or isinstance(value, bool) or value < 0:
                raise Unreadable("%s: %s %s is not a whole number" % (label, name, metric))
    return scenarios


def read_file(path, label):
    try:
        with open(path, encoding="utf-8") as handle:
            return handle.read()
    except (OSError, UnicodeDecodeError) as err:
        raise Unreadable("cannot read %s: %s" % (label, err))


def read_margin(path):
    label = os.path.relpath(path, ROOT) if os.path.abspath(path).startswith(ROOT + os.sep) else path
    try:
        data = json.loads(read_file(path, label))
    except ValueError as err:
        raise Unreadable("%s is not valid JSON: %s" % (label, err))
    margin = data.get("margin_percent") if isinstance(data, dict) else None
    if not isinstance(margin, int) or isinstance(margin, bool) or not 0 <= margin <= 100:
        raise Unreadable("%s must hold a whole margin_percent from 0 to 100" % label)
    return margin


def run_harness():
    """Runs the harness and returns what it printed."""
    run = subprocess.run(
        ["cargo", "run", "--quiet", "--locked", "-p", "vector-runner", "--bin", "resource-snapshot"],
        cwd=os.path.join(ROOT, "soroban"), capture_output=True, text=True, check=False,
    )
    if run.returncode != 0:
        raise Unreadable("the resource harness failed:\n%s" % run.stderr.strip())
    return run.stdout


def compare(committed, fresh, margin):
    """Returns ([problem], number of metrics compared)."""
    problems = []
    compared = 0
    for name in sorted(set(committed) - set(fresh)):
        problems.append("%s: in the snapshot but not in the fresh run" % name)
    for name in sorted(set(fresh) - set(committed)):
        problems.append("%s: in the fresh run but not in the snapshot" % name)
    for name in sorted(set(committed) & set(fresh)):
        for metric in METRICS:
            before, after = committed[name][metric], fresh[name][metric]
            compared += 1
            # Whole numbers: over the margin means 100 * |change| > margin * before.
            if 100 * abs(after - before) <= margin * before:
                continue
            change = "%+d" % (after - before)
            percent = "%+.2f%%" % (100.0 * (after - before) / before) if before else "from zero"
            word = "a regression" if after > before else "an improvement"
            problems.append("%s: %s %d to %d (%s, %s), %s beyond the %d percent margin"
                            % (name, metric, before, after, change, percent, word, margin))
    return problems, compared


def main(argv):
    parser = argparse.ArgumentParser(
        prog="check-resources.py",
        description="Compares a fresh resource snapshot with the committed one.",
    )
    parser.add_argument("--fresh", help="a snapshot to compare instead of running the harness")
    parser.add_argument("--snapshot", default=SNAPSHOT, help="the committed snapshot")
    parser.add_argument("--budget", default=BUDGET, help="the file holding margin_percent")
    parser.add_argument("--update", action="store_true", help="run the harness and write the snapshot")
    args = parser.parse_args(argv[1:])

    try:
        if args.update:
            text = run_harness()
            parse_snapshot(text, "the fresh run")
            with open(args.snapshot, "w", encoding="utf-8", newline="\n") as handle:
                handle.write(text)
            print("check-resources: wrote %s" % args.snapshot)
            return 0
        margin = read_margin(args.budget)
        committed = parse_snapshot(read_file(args.snapshot, args.snapshot), args.snapshot)
        fresh_text = read_file(args.fresh, args.fresh) if args.fresh else run_harness()
        fresh = parse_snapshot(fresh_text, args.fresh or "the fresh run")
    except (Unreadable, OSError) as err:
        print("check-resources: %s" % err, file=sys.stderr)
        return 2

    problems, compared = compare(committed, fresh, margin)
    if problems:
        print("check-resources: %d difference(s) from the snapshot:" % len(problems), file=sys.stderr)
        for problem in problems:
            print("  " + problem, file=sys.stderr)
        print(UPDATE_HINT, file=sys.stderr)
        return 1
    print("check-resources: %d scenario(s), %d metric(s) within the %d percent margin"
          % (len(committed), compared, margin))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
