#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# Fails on literal invisible, bidirectional control, separator and control
# characters in tracked files, file names and symlink targets. Literal
# bidirectional controls can make code read differently from how it runs, and
# none of these characters show in review. Tests and data that need one must
# write it as an escape.
#
# Rejected in every file:
# - every character in the Unicode categories Cf (format, including U+FEFF,
#   U+200B to U+200F, U+202A to U+202E and U+2066 to U+2069), Zl (U+2028) and
#   Zp (U+2029). Categories come from Python's unicodedata tables, not a hand
#   written list;
# - every character in the category Cc (C0 and C1 controls, including ESC,
#   backspace and NEL), except a tab, a line feed, and a carriage return
#   directly before a line feed;
# - the invisible characters those categories miss, listed in INVISIBLE below:
#   the Hangul fillers, variation selectors, U+2800 (braille blank), U+034F
#   (combining grapheme joiner) and the rest of Unicode's default ignorable set.
#
# Rejected unless the file is listed in scripts/invisible-characters-allowlist.txt:
# the lookalike no break space (U+00A0), narrow no break space (U+202F) and
# minus sign (U+2212).
#
# File names and symlink targets follow the same rules, stricter in two ways:
# no control character at all, and the allowlist does not apply to them.
#
# Content is read from the object store, never the working tree, so what is
# checked is what is committed: an attribute cannot make the checkout differ
# from the blob. Each object id cat-file returns must be the one requested. A
# path whose attributes convert content between the blob and the checkout
# (working-tree-encoding, filter, ident, or a text or eol setting that would
# change this blob's bytes) is refused. Every inherited GIT_ variable is dropped
# before calling git. A tracked file missing from the working tree still fails.
#
# Only files whose extension is in BINARY_SIGNATURES count as binary, and only
# when their content starts with that format's signature; a listed name with
# other content fails. What follows the signature is still read, so a valid
# header cannot hide text:
# - when all of it is UTF-8 with no NUL, it is text, and is checked like a file;
# - otherwise every run of valid UTF-8 holding at least MIN_TEXT_IN_BINARY
#   visible characters is checked. Shorter runs turn up in real binary data by
#   chance, and a rejected character alone hides no text.
# Every other file must be valid UTF-8 and is checked in full, and a NUL byte is
# reported as U+0000, so a NUL cannot make a source file skip the check.
# Submodules are skipped apart from their names.
#
# The character classes come with the Python running this, so they can change
# with a runner update. --unicode-version names the version the tables must be,
# and any other version exits 2, so the rejected set never moves silently. CI
# sets it; a local run may leave it out.
#
# Standard library only, no dependencies.
#
# Usage: scripts/check-invisible-characters.py [--unicode-version X.Y.Z] [path inside the repo]
# Exit codes: 0 clean, 1 findings named on stderr, 2 the tree could not be
# listed or read, a path was refused for its attributes, or the Unicode version
# is not the pinned one.

import argparse
import os
import re
import subprocess
import sys
import unicodedata

ALLOWLIST = "scripts/invisible-characters-allowlist.txt"
ALWAYS_REJECTED = {"Cc", "Cf", "Zl", "Zp"}
LOOKALIKES = {0x00A0, 0x202F, 0x2212}

# Invisible characters outside the categories above, as inclusive ranges. This
# is Unicode's Default_Ignorable_Code_Point property, which unicodedata does
# not expose, without the parts that are already Cf, plus the braille blank,
# which is not default ignorable but renders as nothing.
INVISIBLE = (
    (0x034F, 0x034F),  # combining grapheme joiner
    (0x115F, 0x1160),  # Hangul choseong and jungseong fillers
    (0x17B4, 0x17B5),  # Khmer inherent vowels
    (0x180B, 0x180F),  # Mongolian variation selectors and vowel separator
    (0x2800, 0x2800),  # braille pattern blank
    (0x3164, 0x3164),  # Hangul filler
    (0xFE00, 0xFE0F),  # variation selectors
    (0xFFA0, 0xFFA0),  # halfwidth Hangul filler
    (0xFFF0, 0xFFF8),  # reserved default ignorable
    (0xE0000, 0xE0FFF),  # tags, variation selectors supplement and reserved
)

# The only binary file types that are skipped, each with the signatures its
# content must start with. None matches any byte. Add a type here only with a
# reason in review.
BINARY_SIGNATURES = {
    "png": [b"\x89PNG\r\n\x1a\n"],
    "jpg": [b"\xff\xd8\xff"],
    "jpeg": [b"\xff\xd8\xff"],
    "gif": [b"GIF87a", b"GIF89a"],
    # A RIFF container whose form type is WEBP.
    "webp": [b"RIFF" + bytes([0] * 4) + b"WEBP"],
    "ico": [b"\x00\x00\x01\x00"],
    "woff": [b"wOFF"],
    "woff2": [b"wOF2"],
    "wasm": [b"\x00asm"],
    "pdf": [b"%PDF-"],
}
# Byte positions in a signature that match anything (the RIFF chunk size).
ANY_BYTE = {"webp": {4, 5, 6, 7}}

# How many visible characters a run of UTF-8 inside binary data needs before it
# is read as text. In random bytes a two byte rejected character next to eight
# visible ASCII characters turns up about once in 20 million bytes.
MIN_TEXT_IN_BINARY = 8

# One or more well formed UTF-8 characters, leaving out the C0 controls other
# than tab, line feed and carriage return, which is how binary data separates
# runs of text.
TEXT_RUN = re.compile(
    rb"(?:[\t\n\r\x20-\x7e]"
    rb"|[\xc2-\xdf][\x80-\xbf]"
    rb"|\xe0[\xa0-\xbf][\x80-\xbf]"
    rb"|[\xe1-\xec\xee\xef][\x80-\xbf]{2}"
    rb"|\xed[\x80-\x9f][\x80-\xbf]"
    rb"|\xf0[\x90-\xbf][\x80-\xbf]{2}"
    rb"|[\xf1-\xf3][\x80-\xbf]{3}"
    rb"|\xf4[\x80-\x8f][\x80-\xbf]{2})+"
)

MODE_SYMLINK = b"120000"
MODE_GITLINK = b"160000"
ATTRIBUTES = (b"working-tree-encoding", b"filter", b"ident", b"text", b"eol")


class Refused(Exception):
    """The tree cannot be checked, or git gave an answer that cannot be trusted."""


def extension_of(name):
    base = name.rsplit(b"/", 1)[-1]
    if b"." not in base:
        return ""
    return base.rsplit(b".", 1)[1].decode("utf-8", "replace").lower()


def signature_length(extension, data):
    """How many bytes the format's signature takes, or None if it is not there."""
    wildcard = ANY_BYTE.get(extension, set())
    for signature in BINARY_SIGNATURES[extension]:
        if len(data) >= len(signature) and all(
            i in wildcard or data[i] == byte for i, byte in enumerate(signature)
        ):
            return len(signature)
    return None


def is_invisible(point):
    return any(low <= point <= high for low, high in INVISIBLE)


def is_rejected(character, allow_lookalikes=False):
    point = ord(character)
    return (
        unicodedata.category(character) in ALWAYS_REJECTED
        or is_invisible(point)
        or (point in LOOKALIKES and not allow_lookalikes)
    )


def scan_text(text, allow_lookalikes):
    """Each finding is (line, column, code point), with the column counted in
    code points from 1. A byte order mark is kept and reported."""
    findings = []
    lines = text.split("\n")
    for line_number, line in enumerate(lines, start=1):
        for column, character in enumerate(line, start=1):
            if character == "\t":
                continue
            # A carriage return is fine only as the first half of a CRLF pair.
            if character == "\r" and column == len(line) and line_number < len(lines):
                continue
            if is_rejected(character, allow_lookalikes):
                findings.append((line_number, column, ord(character)))
    return findings


def scan_tail(tail, offset, allow_lookalikes):
    """What follows a binary signature. Each finding is (byte offset in the
    file, code point)."""
    try:
        text = tail.decode("utf-8")
    except UnicodeDecodeError:
        text = None
    findings = []
    if text is not None and "\0" not in text:
        runs = [(offset, text, True)]
    else:
        runs = []
        for match in TEXT_RUN.finditer(tail):
            run = match.group().decode("utf-8")
            if sum(1 for c in run if not c.isspace() and not is_rejected(c)) >= MIN_TEXT_IN_BINARY:
                runs.append((offset + match.start(), run, False))
    for start, run, whole in runs:
        at = start
        for index, character in enumerate(run):
            if whole:
                allowed = character in "\t\n" or (
                    character == "\r" and run[index + 1 : index + 2] == "\n"
                )
            else:
                # A lone 0x0D is an ordinary byte in binary data.
                allowed = character in "\t\n\r"
            if not allowed and is_rejected(character, allow_lookalikes):
                findings.append((at, ord(character)))
            at += len(character.encode("utf-8"))
    return findings


def scan(name, data, allow_lookalikes):
    """Returns ("binary", findings in the tail), ("mismatch", extension),
    ("invalid", None) or ("text", findings)."""
    extension = extension_of(name)
    if extension in BINARY_SIGNATURES:
        length = signature_length(extension, data)
        if length is None:
            return "mismatch", extension
        return "binary", scan_tail(data[length:], length, allow_lookalikes)
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        return "invalid", None
    return "text", scan_text(text, allow_lookalikes)


def scan_path_text(raw):
    """Code points rejected in a file name or symlink target, or None when it
    is not valid UTF-8."""
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError:
        return None
    return [ord(c) for c in text if is_rejected(c)]


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
        unsafe = any(is_rejected(c, allow_lookalikes=True) or c in "\"\\" for c in text)
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


def git(args, cwd, stdin=None):
    # Any GIT_ variable could point git at another repository, index, object
    # store or config, so none is inherited.
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    return subprocess.run(
        ["git", "-C", cwd, *args], env=env, input=stdin, capture_output=True, check=False
    )


def list_entries(root):
    """Returns [(mode, object id, path)] from the index, every stage of a
    conflicted file included."""
    listing = git(["ls-files", "-z", "--stage"], root)
    if listing.returncode != 0:
        raise Refused("git ls-files failed in %s" % os.fsdecode(root))
    entries = []
    for record in listing.stdout.split(b"\0"):
        if not record:
            continue
        head, separator, path = record.partition(b"\t")
        fields = head.split(b" ")
        if not separator or len(fields) != 3:
            raise Refused("git ls-files gave a record it should not: %r" % record[:80])
        entries.append((fields[0], fields[1], path))
    return entries


def read_blobs(root, object_ids):
    """Returns {object id: content}, read with git cat-file --batch. Each record
    must be a blob and must name the object id that was asked for."""
    if not object_ids:
        return {}
    answer = git(["cat-file", "--batch"], root, stdin=b"".join(oid + b"\n" for oid in object_ids))
    if answer.returncode != 0:
        raise Refused("git cat-file failed in %s" % os.fsdecode(root))
    out = answer.stdout
    blobs, at = {}, 0
    for wanted in object_ids:
        end = out.find(b"\n", at)
        if end < 0:
            raise Refused("git cat-file ended before object %s" % wanted.decode("ascii", "replace"))
        header = out[at:end].split(b" ")
        if len(header) != 3 or header[0] != wanted or header[1] != b"blob" or not header[2].isdigit():
            raise Refused(
                "git cat-file answered %r for object %s"
                % (out[at:end][:120].decode("ascii", "replace"), wanted.decode("ascii", "replace"))
            )
        size = int(header[2])
        blobs[wanted] = out[end + 1 : end + 1 + size]
        at = end + 1 + size
        if out[at:at + 1] != b"\n" or len(blobs[wanted]) != size:
            raise Refused("git cat-file gave a short record for object %s" % wanted.decode("ascii"))
        at += 1
    if at != len(out):
        raise Refused("git cat-file gave more records than were asked for")
    return blobs


def read_attributes(root, paths):
    """Returns {path: {attribute: value}}, read from the index, for the
    attributes that can convert content."""
    if not paths:
        return {}
    answer = git(
        ["check-attr", "-z", "--cached", "--stdin", *[a.decode("ascii") for a in ATTRIBUTES]],
        root,
        stdin=b"".join(path + b"\0" for path in paths),
    )
    if answer.returncode != 0:
        raise Refused("git check-attr failed in %s" % os.fsdecode(root))
    fields = answer.stdout.split(b"\0")
    if fields and fields[-1] == b"":
        fields.pop()
    if len(fields) != 3 * len(ATTRIBUTES) * len(paths):
        raise Refused("git check-attr gave %d fields for %d paths" % (len(fields), len(paths)))
    attributes = {}
    for i in range(0, len(fields), 3):
        attributes.setdefault(fields[i], {})[fields[i + 1]] = fields[i + 2]
    return attributes


def eol_conversion_changes(text_value, eol_value, data):
    """Whether a text or eol setting would change this blob's bytes between the
    object store and a checkout on any platform."""
    if text_value == b"unset" or (text_value == b"unspecified" and eol_value == b"unspecified"):
        return False
    if text_value == b"auto" and b"\0" in data:
        return False
    if eol_value == b"crlf":
        return re.search(rb"(?<!\r)\n", data) is not None
    return b"\r\n" in data


def attribute_problems(attributes, data):
    """Each is the attribute, as written, that converts this path."""
    problems = []
    for attribute in (b"working-tree-encoding", b"filter", b"ident"):
        value = attributes.get(attribute, b"unspecified")
        if value in (b"unspecified", b"unset"):
            continue
        name = attribute.decode("ascii")
        problems.append(name if value == b"set" else "%s=%s" % (name, value.decode("utf-8", "replace")))
    text_value = attributes.get(b"text", b"unspecified")
    eol_value = attributes.get(b"eol", b"unspecified")
    if eol_conversion_changes(text_value, eol_value, data):
        problems.append("text=%s eol=%s" % (text_value.decode("utf-8", "replace"), eol_value.decode("utf-8", "replace")))
    return problems


def check(root):
    """Returns (findings, refusals, text file count, binary file count)."""
    entries = list_entries(root)
    regular = [e for e in entries if e[0] not in (MODE_SYMLINK, MODE_GITLINK)]
    wanted = []
    for mode, oid, _ in entries:
        if mode != MODE_GITLINK and oid not in wanted:
            wanted.append(oid)
    blobs = read_blobs(root, wanted)
    attributes = read_attributes(root, sorted({e[2] for e in regular}))

    allowed = set()
    for mode, oid, name in regular:
        if name == os.fsencode(ALLOWLIST):
            allowed = parse_allowlist(blobs[oid].decode("utf-8", "replace"))

    problems, refusals = [], []
    text_files = 0
    binary_files = 0
    names = set()
    seen = set()
    for mode, oid, name in entries:
        shown = show_path(name)
        # git lists a conflicted file once per stage; each name is checked
        # once, and each different blob of it once.
        if name not in names:
            names.add(name)
            points = scan_path_text(name)
            if points is None:
                problems.append("%s: file name is not valid UTF-8" % shown)
            else:
                problems.extend("%s: file name: %s" % (shown, code_point(p)) for p in points)
            try:
                os.lstat(os.path.join(root, name))
            except (FileNotFoundError, NotADirectoryError):
                problems.append("%s: tracked but missing from the working tree" % shown)
        if mode == MODE_GITLINK or (name, oid) in seen:
            continue
        seen.add((name, oid))
        data = blobs[oid]
        if mode == MODE_SYMLINK:
            points = scan_path_text(data)
            if points is None:
                problems.append("%s: symlink target is not valid UTF-8" % shown)
            else:
                problems.extend("%s: symlink target: %s" % (shown, code_point(p)) for p in points)
            continue
        for attribute in attribute_problems(attributes.get(name, {}), data):
            refusals.append(
                "%s: attribute %s converts content between the blob and the checkout" % (shown, attribute)
            )
        kind, findings = scan(name, data, name.decode("utf-8", "replace") in allowed)
        if kind == "binary":
            binary_files += 1
            for offset, point in findings:
                problems.append("%s: byte %d: %s in text after the binary signature" % (shown, offset, code_point(point)))
        elif kind == "mismatch":
            problems.append("%s: named .%s but its content is not %s" % (shown, findings, findings))
        elif kind == "invalid":
            problems.append("%s: not valid UTF-8 text" % shown)
        else:
            text_files += 1
            for line, column, point in findings:
                problems.append("%s:%d:%d: %s" % (shown, line, column, code_point(point)))
    return problems, refusals, text_files, binary_files


def main(argv):
    parser = argparse.ArgumentParser(
        prog="check-invisible-characters.py",
        description="Fails on invisible characters in tracked files, file names and symlink targets.",
    )
    parser.add_argument("--unicode-version", help="the Unicode version the character tables must be")
    parser.add_argument("path", nargs="?", help="a path inside the repository (default: this repository)")
    args = parser.parse_args(argv[1:])

    version = unicodedata.unidata_version
    if args.unicode_version is not None:
        if version != args.unicode_version:
            print(
                "check-invisible-characters: this Python has Unicode %s, but %s is pinned; "
                "the set of rejected characters would change, so update the pin on purpose"
                % (version, args.unicode_version),
                file=sys.stderr,
            )
            return 2
        print("check-invisible-characters: Unicode %s, as pinned" % version)

    start = args.path or os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
    top = git(["rev-parse", "--show-toplevel"], start)
    if top.returncode != 0:
        print("check-invisible-characters: %s is not inside a git work tree" % start, file=sys.stderr)
        return 2
    root = os.fsencode(top.stdout.decode("utf-8", "surrogateescape").rstrip("\n"))

    try:
        problems, refusals, text_files, binary_files = check(root)
    except Refused as err:
        print("check-invisible-characters: %s" % err, file=sys.stderr)
        return 2

    if problems:
        print("check-invisible-characters: failed (Unicode %s):" % version, file=sys.stderr)
        for problem in problems:
            print("  " + problem, file=sys.stderr)
        print(
            "Write these characters as escapes. Lookalike spaces and minus signs may be "
            "allowed per file in %s." % ALLOWLIST,
            file=sys.stderr,
        )
    if refusals:
        print("check-invisible-characters: refused:", file=sys.stderr)
        for refusal in refusals:
            print("  " + refusal, file=sys.stderr)
        return 2
    if problems:
        return 1
    print(
        "check-invisible-characters: %d text file(s) checked, %d binary file(s) skipped (Unicode %s)"
        % (text_files, binary_files, version)
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
