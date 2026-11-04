#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Qomira Labs
#
# The checks on the shared test vectors that a JSON Schema cannot express.
# scripts/check-vectors.sh runs this after check-jsonschema has validated the
# same files against spec/vectors/schema.json. It runs even when that
# validation fails, so one run names every problem, which is why every value
# is type checked here before it is used.
#
# Against the spec directory:
#   - every cited rule ID is a rule in MANDATE.md and is not retired (a title
#     starting "Retired:" or an entry in retired-ids.txt);
#   - every expected error name is in errors.json, every event name is in
#     EVENTS.md, and each event lists exactly the fields EVENTS.md gives it.
#
# Inside the files:
#   - JSON numbers are integers only (no fraction, exponent, NaN or Infinity),
#     no object repeats a key, and vector IDs are unique across all files;
#   - an input only one chain's types can express restricts the vector to that
#     chain: a negative amount, an account with no trustline, a token whose
#     transfer returns a value, an attester given as an account's key or an
#     all-zero account or contract (Stellar), an amount above I128_MAX, the
#     zero address, the non-canonical tag, a reentrant test token or a token
#     whose transfer returns no data (Celo);
#   - a tagged destination is an account actor, only an account actor changes
#     whether it can receive, and the mandate, the factory, the tokens and
#     the zero addresses never make a call;
#   - every attestation a request names was made by an earlier attest step,
#     the retired secp256k1 tamper kinds are refused by name, and a
#     wrong_signer tamper never signs with the key it already uses;
#   - a Stellar host_abort is expected only from a request carrying an
#     attestation that fails verification with a 64-byte signature: one whose
#     tamper damages it that way, or an untampered one made for another
#     destination, recipient_ref or signer than its leg, request and the
#     attester in force; and only with ATTESTATION_INVALID on the other chain,
#     so a request_payouts batch can never also expect per-item rejections;
#   - a Stellar host_auth_failure is expected only from a call with no caller
#     argument, or from a call signed by someone other than its caller in a
#     vector that cites the caller authorization rule;
#   - a token that moves the settlement balance is recovered only in a vector
#     restricted to Celo, or restricted to Stellar and expecting
#     TRANSFER_FAILED;
#   - mandate calls and attest steps come after the mandate exists, and time
#     never moves back;
#   - a call that pins the precedence of two checks ("precedence": ["3", "4"])
#     names a check of its function's check order in MANDATE.md and the one
#     right after it, cites both checks' rules, is refused, and is refused the
#     way the earlier check refuses: with one of its errors, with the Stellar
#     host failure its rule names, or for a per-item check with an item's own
#     error rather than the later check's. A pin on a pair PRECEDENCE_EXEMPT
#     names is refused.
#
# Coverage (--require-all, or --areas for some areas): every MANDATE rule that
# is not retired has at least one passing and one failing vector. A vector is
# failing when any call in it is refused on any chain: an error, a per-chain
# override, a payout_request_rejected event or a batch item with a non-zero
# code. Every other vector is passing. CHAINOPS rules bind the off-chain
# services and never count as uncovered.
#
# Parity (--fixture FILE): every case of the attestation parity fixture has a
# vector named parity_<case>, except the cases PARITY_EXEMPT names with the
# reason, and every parity_ vector names a case of the fixture.
#
# Precedence (--precedence): every adjacent pair of checks in every check
# order of MANDATE.md is pinned by at least one call in the files checked,
# except the pairs PRECEDENCE_EXEMPT names with the reason, and every exempt
# pair still matches the spec. When several checks fail at once the earliest
# decides the error, and a pin per adjacent pair is what keeps two contracts
# from each passing every single-rule vector while disagreeing on real input.
#
# Standard library only.
#
# Usage: check-vectors.py --spec DIR [--require-all | --areas A,B,...] [--fixture FILE] [--precedence] FILE...
# Exit codes: 0 every check passes, 1 problems named on stderr, 2 bad usage or
# a spec file that cannot be read.

import json
import os
import re
import sys

FORMAT = "ujira-mandate-vectors/1"
CHAINS = ("stellar", "celo")
I128_MAX = 2**127 - 1
I128_MIN = -(2**127)
U64_MAX = 2**64 - 1

# A 64-byte signature that fails ed25519 verification, which the Stellar host
# turns into a trap the contract cannot catch.
INVALID_SIGNATURE = {
    "wrong_signer",
    "wrong_chain",
    "wrong_mandate",
    "wrong_destination",
    "wrong_tag",
    "wrong_policy",
    "wrong_ref",
    "wrong_valid_until",
    "flipped_bit",
    "non_canonical_s",
}
# A signature that is not 64 bytes, refused with a code before verification.
WRONG_LENGTH = {"empty", "truncated", "short_signature", "long_signature", "extended"}
# The tamper kinds of the secp256k1 attestations, which were never deployed.
RETIRED_TAMPER = {"high_s", "bad_v", "r_zero", "s_zero", "r_order", "r_not_on_curve", "recovers_to_infinity"}
# The test key a wrong_signer attestation is signed with.
WRONG_SIGNER = "#wrong_signer"

# With --fixture, each case of the attestation parity fixture needs a vector
# named PARITY_PREFIX + its case name. The cases below are the ones an attest
# step cannot build. The contract hands every 64-byte signature to the host
# unchanged, so a damaged signature it cannot build is a test of the host's
# verifier, which the spike suite runs against the prototype; the runner's
# attest step is still compared with every case of the fixture.
PARITY_PREFIX = "parity_"
PARITY_EXEMPT = {
    "valid_policy_epoch_u64_max": "the policy epoch reaches the u64 maximum only after as many policy changes",
    "wrong_policy_epoch_signed": "an attest step signs the policy_epoch it carries, so it cannot sign one and carry another",
    "flipped_s_bit": "flipped_bit is the one tamper kind that flips a bit, and it flips one in R",
    "unprefixed_message": "an attest step always signs the prefixed message",
    "zero_signature": "no tamper kind replaces the whole signature",
    "identity_r_zero_s": "no tamper kind replaces the whole signature",
    "small_order_attester_key": "no tamper kind writes the identity point and a zero S as the signature",
}

# With --precedence, each adjacent pair of checks needs a pinned call, except
# these pairs, which no input can make fail together. Each is keyed by the
# operation and its two check labels, and names the two rules it was written
# against, so a renumbered or rewritten check order makes the entry stale
# instead of silently exempting another pair.
#
# The attester key holds no role (MANDATE-ROLE-018) on every path that sets the
# attester or adds the principal, the fee address or a role member, so an
# account that already conflicts with a role never has the attester's key.
ATTESTER_KEY_HELD = (
    "an account that conflicts is the principal, the fee address or a role member, and the attester's key "
    "never matches any of those, since every path that sets either refuses the overlap"
)
PRECEDENCE_EXEMPT = {
    ("request_payout", "16", "17"): (
        "MANDATE-RECIPIENT-003",
        "MANDATE-RECIPIENT-004",
        "check 16 applies only in ALLOWLIST mode and check 17 only in ATTESTED and BOTH",
    ),
    ("request_payout", "19", "20"): (
        "MANDATE-RECIPIENT-006",
        "MANDATE-RECIPIENT-007",
        "an expired attestation has valid_until at or before now, so its window is never too long",
    ),
    ("add_role", "4", "5"): (
        "MANDATE-ROLE-017",
        "MANDATE-ROLE-009",
        "the zero address is never the principal, the fee address or a role member, so adding it breaks no separation",
    ),
    ("add_role", "5", "6"): ("MANDATE-ROLE-009", "MANDATE-ROLE-018", ATTESTER_KEY_HELD),
    ("remove_role", "3", "4"): (
        "MANDATE-ROLE-016",
        "MANDATE-APPROVAL-006",
        "check 4 applies only to an approver that is in the set, which is when check 3 passes",
    ),
    ("remove_role", "4", "5"): (
        "MANDATE-APPROVAL-006",
        "MANDATE-APPROVAL-008",
        "check 4 removes one approver among several and check 5 the only one",
    ),
    ("remove_role", "5", "6"): (
        "MANDATE-APPROVAL-008",
        "MANDATE-RECOVERYSET-002",
        "check 5 applies to role 2 and check 6 to role 5",
    ),
    ("propose_principal", "2", "3"): (
        "MANDATE-ROLE-017",
        "MANDATE-PRINCIPAL-001",
        "the zero address holds no role and is not the fee address",
    ),
    ("propose_principal", "3", "4"): ("MANDATE-PRINCIPAL-001", "MANDATE-ROLE-018", ATTESTER_KEY_HELD),
    ("accept_principal", "3", "4"): ("MANDATE-PRINCIPAL-006", "MANDATE-ROLE-018", ATTESTER_KEY_HELD),
    ("recovery_rotate_agent", "5", "6"): ("MANDATE-RECOVERYSET-006", "MANDATE-ROLE-018", ATTESTER_KEY_HELD),
}

# Functions that take a caller argument, which the call authorizes first.
CALLER_ARG = {
    "request_payout",
    "request_payouts",
    "approve",
    "reject",
    "cancel",
    "cancel_batch",
    "execute",
    "execute_batch",
    "pause",
    "sweep",
    "recovery_revoke",
    "recovery_rotate_agent",
}
VIEWS = {"version", "config", "state", "payout", "daily_used", "is_allowed"}
REQUESTS = {"request_payout", "request_payouts"}
CALLER_AUTH_RULE = "MANDATE-ROLE-019"

# Reserved actors that are not accounts and can never make a call.
NOT_ACCOUNTS = {"mandate", "factory", "asset", "other_token", "zero", "zero_contract"}
# The Stellar account with an all-zero key is an account, so it can take a
# tag, but nobody holds its key, so it never makes a call either.
NEVER_CALL = NOT_ACCOUNTS | {"zero_account"}
# Addresses nobody controls that only Stellar can name.
STELLAR_ZERO = {"zero_account", "zero_contract"}
# Keys whose values are amounts wherever they appear in a vector.
AMOUNT_KEYS = {
    "total",
    "fee",
    "amount",
    "total_budget",
    "per_payment_cap",
    "per_recipient_daily_cap",
    "approval_threshold",
    "spent",
    "pending",
    "committed",
    "balance",
    "free",
    "frozen",
    "used",
}
# Keys whose values are prose or names, never actors.
TEXT_KEYS = {"description", "id", "rules"}

problems = 0


def report(where, msg):
    global problems
    print("check-vectors: %s: %s" % (where, msg), file=sys.stderr)
    problems += 1


def fail_usage(msg):
    print("check-vectors: " + msg, file=sys.stderr)
    sys.exit(2)


def read_text(path):
    try:
        with open(path, "rb") as fh:
            return fh.read().decode("utf-8")
    except (OSError, UnicodeDecodeError) as err:
        fail_usage("cannot read %s: %s" % (path, err))
    return ""


def load_spec(spec_dir):
    """Returns (rules {id: title}, retired ids, error names, events {name: [fields]})."""
    rules = {}
    fence = False
    for line in read_text(os.path.join(spec_dir, "MANDATE.md")).split("\n"):
        if line.startswith("```"):
            fence = not fence
            continue
        m = None if fence else re.match(r"^#### ((?:MANDATE|CHAINOPS)-[A-Z]+-[0-9]{3}) (.*)$", line)
        if m:
            rules[m.group(1)] = m.group(2)
    if not rules:
        fail_usage("no rules found in %s" % os.path.join(spec_dir, "MANDATE.md"))
    retired = {rid for rid, title in rules.items() if title.startswith("Retired:")}
    retired_path = os.path.join(spec_dir, "retired-ids.txt")
    if os.path.exists(retired_path):
        for line in read_text(retired_path).split("\n"):
            entry = line.split("#", 1)[0].strip()
            if entry:
                retired.add(entry)

    errors = set()
    try:
        for entry in json.loads(read_text(os.path.join(spec_dir, "errors.json"))):
            errors.add(entry["name"])
    except (ValueError, TypeError, KeyError) as err:
        fail_usage("cannot read the error names from errors.json: %s" % err)

    events = {}
    for line in read_text(os.path.join(spec_dir, "EVENTS.md")).split("\n"):
        if not line.startswith("| `"):
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) < 2:
            continue
        fields = [f.strip() for f in cells[1].split(",") if f.strip()]
        for name in re.findall(r"`([a-z][a-z0-9_]*)`", cells[0]):
            events[name] = fields
    if not events:
        fail_usage("no events found in %s" % os.path.join(spec_dir, "EVENTS.md"))
    return rules, retired, errors, events


def load_orders(spec_dir, errors):
    """Returns (orders, outcomes). orders maps each operation to its numbered
    checks in order, as (label, rule ID) pairs, from the check order blocks of
    MANDATE.md. outcomes maps each rule to what its Error line says it can
    return: the error names, whether Stellar fails it in the host instead
    (authorization or a trap), and whether it hands off to per-item checks.
    check-spec-ids.sh --check-order proves the blocks are well formed, so
    this only reads them."""
    orders, outcomes = {}, {}
    fence, current, rule = False, None, None
    for line in read_text(os.path.join(spec_dir, "MANDATE.md")).split("\n"):
        if line.startswith("```"):
            fence = not fence
            continue
        if fence:
            continue
        heading = re.match(r"^#### ((?:MANDATE|CHAINOPS)-[A-Z]+-[0-9]{3}) ", line)
        if heading:
            rule = heading.group(1)
            continue
        if line.startswith("#"):
            rule = None
        if rule and line.startswith("- Error:"):
            text = line[len("- Error:"):].strip()
            outcomes[rule] = {
                "names": [n for n in re.findall(r"`([A-Z][A-Z0-9_]*)`", text) if n in errors],
                "host_auth": "host authorization failure" in text,
                "host_abort": "host trap" in text,
                "per_item": text.startswith("per item"),
            }
        opened = re.match(r"^<!-- check-order: ([a-z_]+) -->$", line)
        if opened:
            current = opened.group(1)
            orders[current] = []
            continue
        if line == "<!-- /check-order -->":
            current = None
            continue
        if current is not None:
            step = re.match(r"^([0-9]+[a-z]?)\. ", line)
            cited = re.findall(r"MANDATE-[A-Z]+-[0-9]{3}", line)
            if step and cited:
                orders[current].append((step.group(1), cited[0]))
    return orders, outcomes


def load_vectors(path):
    """Parses one file, reporting repeated keys and numbers that are not integers."""
    found = []

    def pairs(items):
        obj = {}
        for key, value in items:
            if key in obj:
                found.append("the key %s appears twice in one object" % json.dumps(key))
            obj[key] = value
        return obj

    def no_float(text):
        found.append("%s is not an integer; amounts are decimal strings and counts are JSON integers, never fractions" % text)
        return 0

    def no_constant(text):
        found.append("%s is not valid JSON" % text)
        return 0

    try:
        with open(path, "rb") as fh:
            raw = fh.read().decode("utf-8")
    except (OSError, UnicodeDecodeError) as err:
        report(path, "cannot read the file: %s" % err)
        return None
    try:
        data = json.loads(raw, object_pairs_hook=pairs, parse_float=no_float, parse_constant=no_constant)
    except ValueError as err:
        report(path, "is not valid JSON: %s" % err)
        return None
    for msg in found:
        report(path, msg)
    return data


def walk(node, path="", parents=()):
    """Yields (path, key, value, parent keys) for every member of every object
    and array. Array members have the key None."""
    if isinstance(node, dict):
        for key, value in node.items():
            sub = "%s.%s" % (path, key) if path else key
            yield sub, key, value, parents
            yield from walk(value, sub, parents + (key,))
    elif isinstance(node, list):
        for i, value in enumerate(node):
            sub = "%s[%d]" % (path, i)
            yield sub, None, value, parents
            yield from walk(value, sub, parents + (None,))


def decimal(value):
    if isinstance(value, str) and re.fullmatch(r"0|-?[1-9][0-9]*", value):
        return int(value)
    return None


def refuses(expect):
    """True when a call is refused on some chain, wholly or for a batch item."""
    if "error" in expect:
        return True
    per_chain = expect.get("per_chain")
    if isinstance(per_chain, dict) and per_chain:
        return True
    if "payout_request_rejected" in event_names(expect):
        return True
    result = expect.get("return")
    if isinstance(result, list):
        return any(isinstance(item, dict) and item.get("code") not in (0, None) for item in result)
    return False


def event_names(expect):
    events = expect.get("events")
    if not isinstance(events, list):
        return []
    return [e.get("name") for e in events if isinstance(e, dict)]


class Vector:
    def __init__(self, path, index, data, spec, seen_ids, orders):
        self.data = data
        self.rules, self.retired, self.errors, self.events = spec
        self.orders, self.outcomes = orders
        # Every (operation, earlier check, later check) this vector pins.
        self.pins = []
        vid = data.get("id")
        self.id = vid if isinstance(vid, str) else "#%d" % (index + 1)
        self.where = "%s: vector %s" % (path, self.id)
        if isinstance(vid, str):
            if vid in seen_ids:
                self.problem("the ID %s is also used in %s; vector IDs are unique across every file" % (vid, seen_ids[vid]))
            else:
                seen_ids[vid] = path
        applies = data.get("applies_to")
        chains = {c for c in applies if c in CHAINS} if isinstance(applies, list) else set()
        self.runs_on = chains or set(CHAINS)
        setup = data.get("setup")
        self.setup = setup if isinstance(setup, dict) else {}
        self.cited = set()
        self.refused = False
        self.movers = set()
        config = self.setup.get("mandate")
        self.attester = config.get("attester") if isinstance(config, dict) else None

    def problem(self, msg, step=None):
        report(self.where if step is None else "%s step %d" % (self.where, step), msg)

    def only_on(self, chain, what, why):
        """An input only one chain can express needs the vector restricted to it."""
        if self.runs_on != {chain}:
            self.problem(
                "%s, which only %s can express (%s); restrict the vector with applies_to [\"%s\"] and "
                "skip_reason unrepresentable_input" % (what, chain.capitalize(), why, chain)
            )

    def check(self):
        self.check_rules()
        self.check_values()
        self.check_setup()
        steps = self.data.get("steps")
        if isinstance(steps, list):
            self.check_steps(steps)

    def check_rules(self):
        rules = self.data.get("rules")
        for rid in rules if isinstance(rules, list) else []:
            if not isinstance(rid, str):
                continue
            if rid not in self.rules:
                self.problem("cites %s, which is not a rule in MANDATE.md" % rid)
            elif rid in self.retired:
                self.problem("cites %s, which is retired; cite the rule that replaced it" % rid)
            else:
                self.cited.add(rid)

    def check_values(self):
        for path, key, value, parents in walk(self.data):
            if key in TEXT_KEYS:
                continue
            # Balances are keyed by token, then by actor.
            balance = len(parents) >= 2 and parents[-2] == "balances"
            if value == "zero" or (balance and key == "zero"):
                self.only_on("celo", "%s is the zero address" % path, "Stellar has no zero address")
            if (isinstance(value, str) and value in STELLAR_ZERO) or (balance and key in STELLAR_ZERO):
                self.only_on(
                    "stellar",
                    "%s is an all-zero Stellar address" % path,
                    "only a Stellar address is a 32-byte account key or contract ID",
                )
            if key in AMOUNT_KEYS or balance:
                self.check_amount(path, value)
            if isinstance(value, dict) and "has_tag" in value:
                self.only_on("celo", "%s is the non-canonical tag form" % path, "a Stellar destination cannot mark a tag absent and still carry one")
            if isinstance(value, dict) and set(value) == {"account_key"}:
                self.only_on("stellar", "%s is the ed25519 key of an account" % path, "only a Stellar account address is an ed25519 key")
            if isinstance(value, dict) and set(value) == {"address", "tag"} and value.get("tag") is not None:
                address = value.get("address")
                if isinstance(address, str) and not self.is_account(address):
                    self.problem("%s gives %s a tag, but only an account actor can take one (the runner builds an M address on Stellar)" % (path, address))

    def check_amount(self, path, value):
        number = decimal(value)
        if number is None:
            return
        if number < I128_MIN:
            self.problem("%s is %s, below the smallest i128, which no chain can express" % (path, value))
        elif number < 0:
            self.only_on("stellar", "%s is the negative amount %s" % (path, value), "Celo amounts are unsigned")
        elif number > I128_MAX:
            self.only_on("celo", "%s is %s, above I128_MAX" % (path, value), "Stellar amounts are i128")

    def is_account(self, actor):
        declared = self.setup.get("actors")
        if isinstance(declared, dict):
            spec = declared.get(actor)
            if isinstance(spec, dict) and spec.get("kind") == "contract":
                return False
        return actor not in NOT_ACCOUNTS

    def check_setup(self):
        tokens = self.setup.get("tokens")
        for name, spec in tokens.items() if isinstance(tokens, dict) else []:
            if not isinstance(spec, dict):
                continue
            if spec.get("kind") == "reentrant":
                self.only_on(
                    "celo",
                    "token %s is a reentrant test token" % name,
                    "the Stellar host refuses any call into a contract already on the call stack",
                )
            elif spec.get("kind") == "moves_settlement":
                self.movers.add(name)
            elif spec.get("kind") == "returns_value":
                self.only_on(
                    "stellar",
                    "token %s returns a value from transfer" % name,
                    "only a Stellar transfer returns nothing, so only there is a returned value unexpected",
                )
            elif spec.get("kind") == "returns_no_data":
                self.only_on(
                    "celo",
                    "token %s returns no data from transfer" % name,
                    "only an EVM call can return no data at all",
                )
        config = self.setup.get("mandate")
        if isinstance(config, dict) and config.get("recipient_mode") == "ATTESTED" and self.setup.get("allowlist"):
            self.problem("setup has an allowlist, but an ATTESTED mandate has none")

    def check_steps(self, steps):
        now = decimal(self.setup.get("now"))
        has_mandate = isinstance(self.setup.get("mandate"), dict)
        attestations = {}
        for n, step in enumerate(steps, 1):
            if not isinstance(step, dict):
                continue
            kind = step.get("step")
            if kind == "advance_time":
                seconds = decimal(step.get("seconds"))
                if now is not None and seconds is not None:
                    now += seconds
                    if now > U64_MAX:
                        self.problem("time passes the largest u64", n)
            elif kind == "set_time":
                target = decimal(step.get("now"))
                if now is not None and target is not None and target < now:
                    self.problem("set_time moves time back from %d to %d; ledger time never goes back" % (now, target), n)
                if target is not None:
                    now = target
            elif kind == "attest":
                self.check_attest(step, n, has_mandate, attestations)
            elif kind == "set_receiving":
                self.check_receiving(step, n)
            elif kind == "call":
                if self.check_call(step, n, has_mandate, attestations):
                    has_mandate = True

    def check_receiving(self, step, n):
        account = step.get("account")
        if isinstance(account, str) and not self.is_account(account):
            self.problem(
                "set_receiving names %s, which is not an account; only an account holds a trustline "
                "or is blocked by the Celo test token" % account,
                n,
            )
        if step.get("state") == "no_trustline":
            self.only_on("stellar", "step %d leaves %s with no trustline" % (n, account), "only Stellar has trustlines")

    def check_attest(self, step, n, has_mandate, attestations):
        if not has_mandate:
            self.problem("attests before the mandate exists; an attestation binds the mandate's address", n)
        name = step.get("name")
        if isinstance(name, str):
            if name in attestations:
                self.problem("makes a second attestation named %s" % name, n)
            attestations[name] = {
                "tamper": step.get("tamper"),
                "signer": step.get("signer"),
                "recipient_ref": step.get("recipient_ref"),
                "destination": step.get("destination"),
            }
        tamper = step.get("tamper")
        if tamper == "wrong_signer" and step.get("signer") == WRONG_SIGNER:
            self.problem("wrong_signer signs with %s, so the signer cannot be %s too" % (WRONG_SIGNER, WRONG_SIGNER), n)
        if isinstance(tamper, str):
            retired = {tamper} & RETIRED_TAMPER
        elif isinstance(tamper, dict):
            retired = set(tamper) & RETIRED_TAMPER
        else:
            retired = set()
        for kind in sorted(retired):
            self.problem(
                "tamper %s is a retired secp256k1 kind; attestations are ed25519 signatures, so use one of %s"
                % (kind, ", ".join(sorted(INVALID_SIGNATURE | WRONG_LENGTH))),
                n,
            )

    def check_call(self, step, n, has_mandate, attestations):
        """Checks one call and returns True when it creates the mandate."""
        fn = step.get("fn") if isinstance(step.get("fn"), str) else ""
        for role in ("caller", "signed_by"):
            actor = step.get(role)
            if isinstance(actor, str) and actor in NEVER_CALL:
                self.problem("%s %s can never make a call" % (role, actor), n)
        if "signed_by" in step and fn not in CALLER_ARG:
            self.problem("signed_by is only for calls that take a caller argument, and %s takes none" % fn, n)
        if fn != "create" and not has_mandate:
            self.problem("calls %s before the mandate exists; add setup.mandate or a create step that succeeds" % fn, n)
        expect = step.get("expect") if isinstance(step.get("expect"), dict) else {}
        args = step.get("args") if isinstance(step.get("args"), dict) else {}

        self.check_expect(expect, n)
        used = self.attestations_used(fn, args, n, attestations)
        self.check_overrides(step, fn, expect, used, n)
        if "precedence" in step:
            self.check_precedence(step.get("precedence"), fn, expect, n)
        token = args.get("token")
        if fn == "recover_token" and token in self.movers:
            stellar_transfer_failed = self.runs_on == {"stellar"} and expect.get("error") == "TRANSFER_FAILED"
            if self.runs_on != {"celo"} and not stellar_transfer_failed:
                self.problem(
                    "recovers %s, a token that moves the settlement balance; restrict the vector to celo, "
                    "or to stellar with TRANSFER_FAILED expected there" % token,
                    n,
                )
        if refuses(expect):
            self.refused = True
        succeeds = "error" not in expect and expect.get("ok") is True and not expect.get("per_chain")
        if fn == "set_attestation_policy" and succeeds:
            self.attester = args.get("attester")
        if fn == "create" and "error" not in expect and expect.get("ok") is True:
            if has_mandate:
                self.problem("creates a second mandate; a vector has one mandate", n)
            config = args.get("config")
            self.attester = config.get("attester") if isinstance(config, dict) else None
            return True
        return False

    def check_expect(self, expect, n):
        error = expect.get("error")
        if isinstance(error, str) and error not in self.errors:
            self.problem("expects error %s, which is not in errors.json" % error, n)
        events = expect.get("events")
        for i, event in enumerate(events if isinstance(events, list) else [], 1):
            name = event.get("name") if isinstance(event, dict) else None
            if not isinstance(name, str):
                continue
            if name not in self.events:
                self.problem("expects event %s, which is not in EVENTS.md" % name, n)
                continue
            fields = event.get("fields")
            if not isinstance(fields, dict):
                continue
            want = self.events[name]
            if sorted(fields) != sorted(want):
                self.problem(
                    "event %d (%s) has the fields %s, but EVENTS.md gives it %s"
                    % (i, name, ", ".join(sorted(fields)) or "none", ", ".join(want) or "none"),
                    n,
                )
            code = fields.get("code")
            if isinstance(code, str) and code not in self.errors:
                self.problem("event %d (%s) has the code %s, which is not in errors.json" % (i, name, code), n)
        result = expect.get("return")
        for item in result if isinstance(result, list) else []:
            code = item.get("code") if isinstance(item, dict) else None
            if isinstance(code, str) and code not in self.errors:
                self.problem("a batch item has the code %s, which is not in errors.json" % code, n)

    def attestations_used(self, fn, args, n, attestations):
        """Returns (tamper, verifiable) for every attestation the call's
        requests carry. An attestation can only verify when nothing tampered
        with it and it was made for the leg at its own index, for the request's
        recipient_ref, and by the key that is the attester at this call. A label
        is the only signer the checker can compare, so a raw or account_key
        attester never counts as the signer."""
        if fn == "request_payout":
            reqs = [args.get("req")]
        elif fn == "request_payouts" and isinstance(args.get("reqs"), list):
            reqs = args.get("reqs")
        else:
            return []
        used = []
        for req in reqs:
            if not isinstance(req, dict):
                continue
            names = req.get("attestations")
            legs = req.get("legs") if isinstance(req.get("legs"), list) else []
            for i, name in enumerate(names if isinstance(names, list) else []):
                if not isinstance(name, str):
                    continue
                if name not in attestations:
                    self.problem("uses the attestation %s, which no earlier attest step made" % name, n)
                    continue
                made = attestations[name]
                leg = legs[i] if i < len(legs) and isinstance(legs[i], dict) else {}
                verifiable = (
                    made["tamper"] is None
                    and made["destination"] == leg.get("destination")
                    and made["recipient_ref"] == req.get("recipient_ref")
                    and isinstance(self.attester, str)
                    and made["signer"] == self.attester
                )
                used.append((made["tamper"], verifiable))
        return used

    def check_overrides(self, step, fn, expect, used, n):
        per_chain = expect.get("per_chain")
        if not isinstance(per_chain, dict):
            return
        for chain in per_chain:
            if chain in CHAINS and chain not in self.runs_on:
                self.problem("has a per_chain expectation for %s, but the vector does not run there" % chain, n)
        stellar = per_chain.get("stellar")
        if not isinstance(stellar, dict):
            return
        if "host_abort" in stellar:
            self.check_host_abort(fn, expect, used, n)
        if "host_auth_failure" in stellar:
            self.check_host_auth_failure(step, fn, n)

    def check_host_abort(self, fn, expect, used, n):
        if fn not in REQUESTS:
            self.problem("expects a Stellar host_abort from %s; only a request carrying an attestation with an invalid signature traps" % fn, n)
        elif not any(t in INVALID_SIGNATURE or (t is None and not ok) for t, ok in used):
            tampers = sorted({t for t, _ in used if isinstance(t, str)})
            self.problem(
                "expects a Stellar host_abort, but no attestation it carries has an invalid-signature tamper (%s), "
                "and each untampered one was made for its own leg and recipient_ref by the attester; "
                "a signature that is not 64 bytes is refused with ATTESTATION_INVALID before verification"
                % (", ".join(tampers) if tampers else "none is tampered"),
                n,
            )
        if fn == "request_payouts" and "error" not in expect:
            also = ""
            if "payout_request_rejected" in event_names(expect):
                also = ", so it cannot also expect payout_request_rejected events"
            self.problem(
                "a request_payouts call with a Stellar host_abort fails as a whole%s; expect "
                "{ \"error\": \"ATTESTATION_INVALID\", \"events\": [] } for the other chain" % also,
                n,
            )
        elif expect.get("error") != "ATTESTATION_INVALID":
            self.problem(
                "a Stellar host_abort needs ATTESTATION_INVALID as the other chain's error, not %s"
                % (expect.get("error") or "success"),
                n,
            )

    def check_host_auth_failure(self, step, fn, n):
        if fn in VIEWS:
            self.problem("expects a host_auth_failure from the view %s, which needs no authorization" % fn, n)
            return
        if fn not in CALLER_ARG:
            return
        if CALLER_AUTH_RULE not in self.cited:
            self.problem(
                "expects a host_auth_failure from %s, which takes a caller argument; only a vector that cites %s may"
                % (fn, CALLER_AUTH_RULE),
                n,
            )
        signed_by = step.get("signed_by")
        if not isinstance(signed_by, str) or signed_by == step.get("caller"):
            self.problem("expects a host_auth_failure, but the call is signed by its own caller; set signed_by to another actor", n)

    def check_precedence(self, pin, fn, expect, n):
        """A pin says this call fails two adjacent checks of its function's
        check order and that the earlier one decides the outcome."""
        if not (isinstance(pin, list) and len(pin) == 2 and all(isinstance(p, str) for p in pin)):
            self.problem("precedence must name two checks, such as [\"3\", \"4\"]", n)
            return
        first, then = pin
        if "error" not in expect:
            self.problem("a precedence pin needs a call that is refused, with the error of the earlier check", n)
            return
        if fn not in self.orders:
            self.problem("pins checks of %s, but %s has no check order in the spec" % (fn, fn), n)
            return
        labels = [label for label, _ in self.orders[fn]]
        rule_at = dict(self.orders[fn])
        for label in pin:
            if label not in rule_at:
                self.problem("pins %s check %s, but %s has no check %s" % (fn, label, fn, label), n)
                return
        if labels.index(then) != labels.index(first) + 1:
            self.problem(
                "pins %s checks %s and %s, but %s checks %s and %s are not adjacent; "
                "a pin names a check and the one right after it" % (fn, first, then, fn, first, then),
                n,
            )
            return
        exempt = PRECEDENCE_EXEMPT.get((fn, first, then))
        if exempt is not None and exempt[:2] == (rule_at[first], rule_at[then]):
            self.problem("%s checks %s and %s can never fail together (%s), so no call can pin them" % (fn, first, then, exempt[2]), n)
            return
        for rid in (rule_at[first], rule_at[then]):
            if rid not in self.cited:
                self.problem("pins %s checks %s and %s, so it must cite %s" % (fn, first, then, rid), n)
        outcome = self.outcomes.get(rule_at[first], {})
        later = self.outcomes.get(rule_at[then], {})
        where = "%s check %s (%s)" % (fn, first, rule_at[first])
        error = expect.get("error")
        names = outcome.get("names") or []
        if names and error not in names:
            self.problem(
                "expects %s, but the earlier of the pinned checks, %s, returns %s" % (error, where, " or ".join(names)),
                n,
            )
        if outcome.get("per_item") and error in (later.get("names") or []):
            self.problem(
                "expects %s, the error of %s check %s, but %s hands off to an item's own checks, whose error comes first"
                % (error, fn, then, where),
                n,
            )
        stellar = expect.get("per_chain", {}).get("stellar") if isinstance(expect.get("per_chain"), dict) else None
        host = outcome.get("host_auth") or outcome.get("host_abort")
        if "stellar" in self.runs_on:
            if outcome.get("host_auth") and not (isinstance(stellar, dict) and "host_auth_failure" in stellar):
                self.problem("%s fails host authorization on Stellar; expect per_chain stellar host_auth_failure" % where, n)
            elif outcome.get("host_abort") and not (isinstance(stellar, dict) and "host_abort" in stellar):
                self.problem("%s traps in the Stellar host; expect per_chain stellar host_abort" % where, n)
            elif not host and isinstance(stellar, dict):
                self.problem("expects a Stellar host failure, but %s is a mandate error on every chain" % where, n)
        elif host and not names:
            self.problem("%s fails only in the Stellar host, so the vector must run on stellar" % where, n)
        self.pins.append((fn, first, then, error in names and error in (later.get("names") or [])))


def rule_area(rid):
    return rid.split("-")[1]


def coverage_gaps(spec, vectors, areas):
    """Returns (rules checked, gaps). Exits 2 when an area has no MANDATE rule."""
    rules, retired, _, _ = spec
    live = sorted(r for r in rules if r.startswith("MANDATE-") and r not in retired)
    if areas is not None:
        known = {rule_area(r) for r in rules if r.startswith("MANDATE-")}
        unknown = [a for a in areas if a not in known]
        if unknown:
            fail_usage("--areas names %s, which has no MANDATE rule; CHAINOPS rules are never covered by vectors" % ", ".join(unknown))
        live = [r for r in live if rule_area(r) in areas]
    passing, failing = set(), set()
    for vector in vectors:
        (failing if vector.refused else passing).update(vector.cited)
    gaps = []
    for rid in live:
        lacks = [kind for kind, have in (("passing", passing), ("failing", failing)) if rid not in have]
        if lacks:
            gaps.append("%s has no %s vector" % (rid, " and no ".join(lacks)))
    return live, gaps


def parity_gaps(fixture_path, vectors):
    """Returns (case names, gaps): every case of the attestation parity
    fixture needs a vector named parity_<case>, unless it is in PARITY_EXEMPT."""
    try:
        cases = [c["name"] for c in json.loads(read_text(fixture_path))["cases"]]
    except (ValueError, TypeError, KeyError) as err:
        fail_usage("cannot read the case names from %s: %s" % (fixture_path, err))
    if not cases or not all(isinstance(c, str) for c in cases):
        fail_usage("%s has no named cases" % fixture_path)
    ids = {v.data.get("id") for v in vectors}
    gaps = []
    for case in cases:
        vid = PARITY_PREFIX + case
        if case in PARITY_EXEMPT and vid in ids:
            gaps.append("%s now has the vector %s; drop it from PARITY_EXEMPT in scripts/check-vectors.py" % (case, vid))
        elif case not in PARITY_EXEMPT and vid not in ids:
            gaps.append("case %s of %s has no vector %s" % (case, fixture_path, vid))
    for vid in sorted(i for i in ids if isinstance(i, str) and i.startswith(PARITY_PREFIX)):
        if vid[len(PARITY_PREFIX):] not in cases:
            gaps.append("vector %s names no case of %s" % (vid, fixture_path))
    return cases, gaps


def precedence_gaps(spec_dir, orders, vectors):
    """Returns (pairs, exempt, pinned, same_error, gaps): every adjacent pair
    of every check order needs a pinned call, unless PRECEDENCE_EXEMPT names
    it, and every exemption for an operation the spec has must still match
    its two checks. same_error counts the pinned pairs whose two checks share
    the error that was pinned, where only the code path shows the order."""
    if not any(orders.values()):
        fail_usage("--precedence needs check orders, and there are no check orders in %s" % os.path.join(spec_dir, "MANDATE.md"))
    pinned, shared = set(), set()
    for vector in vectors:
        for fn, first, then, same in vector.pins:
            pinned.add((fn, first, then))
            if same:
                shared.add((fn, first, then))
    pairs, exempt, gaps = 0, [], []
    for (fn, first, then), (rule_a, rule_b, _) in sorted(PRECEDENCE_EXEMPT.items()):
        if fn not in orders:
            continue
        steps = orders[fn]
        if not any(steps[i] == (first, rule_a) and steps[i + 1] == (then, rule_b) for i in range(len(steps) - 1)):
            gaps.append(
                "the precedence exemption for %s checks %s and %s (%s, %s) no longer matches the spec; "
                "update or drop it in PRECEDENCE_EXEMPT in scripts/check-vectors.py" % (fn, first, then, rule_a, rule_b)
            )
    for fn in sorted(orders):
        steps = orders[fn]
        for (first, rule_a), (then, rule_b) in zip(steps, steps[1:]):
            pairs += 1
            entry = PRECEDENCE_EXEMPT.get((fn, first, then))
            if entry is not None and entry[:2] == (rule_a, rule_b):
                exempt.append("%s %s-%s" % (fn, first, then))
            elif (fn, first, then) not in pinned:
                gaps.append(
                    "%s checks %s and %s (%s, then %s) have no precedence vector: no call fails both and gets the "
                    "outcome of check %s" % (fn, first, then, rule_a, rule_b, first)
                )
    return pairs, exempt, len(pinned), len(shared), gaps


def main(argv):
    spec_dir, require_all, areas, files, fixture = None, False, None, [], None
    precedence = False
    args = iter(argv[1:])
    for arg in args:
        if arg == "--spec":
            spec_dir = next(args, None)
        elif arg == "--fixture":
            fixture = next(args, None)
            if not fixture:
                fail_usage("--fixture needs the attestation parity fixture file")
        elif arg == "--require-all":
            require_all = True
        elif arg == "--precedence":
            precedence = True
        elif arg == "--areas":
            value = next(args, None) or ""
            areas = sorted({a for a in value.split(",") if a})
            if not areas or any(not re.fullmatch(r"[A-Z]+", a) for a in areas):
                fail_usage("--areas takes a comma separated list of rule areas, such as PAUSE,FEE")
        elif arg.startswith("-"):
            fail_usage("unknown option %s" % arg)
        else:
            files.append(arg)
    if spec_dir is None or not files:
        fail_usage("usage: check-vectors.py --spec DIR [--require-all | --areas A,B] [--fixture FILE] [--precedence] FILE...")
    if require_all and areas is not None:
        fail_usage("--require-all and --areas cannot be combined")

    spec = load_spec(spec_dir)
    orders = load_orders(spec_dir, spec[2])
    seen_ids = {}
    vectors = []
    for path in files:
        data = load_vectors(path)
        if data is None:
            continue
        if not isinstance(data, dict):
            report(path, "must hold a JSON object")
            continue
        if data.get("format") != FORMAT:
            report(path, "format must be %s" % FORMAT)
        items = data.get("vectors")
        for index, item in enumerate(items if isinstance(items, list) else []):
            if isinstance(item, dict):
                vector = Vector(path, index, item, spec, seen_ids, orders)
                vector.check()
                vectors.append(vector)

    covered = None
    if require_all or areas is not None:
        covered, gaps = coverage_gaps(spec, vectors, areas)
        for gap in gaps:
            report("coverage", gap)
    cases = None
    if fixture is not None:
        cases, gaps = parity_gaps(fixture, vectors)
        for gap in gaps:
            report("parity", gap)
    ordered = None
    if precedence:
        pairs, exempt, pinned, shared, gaps = precedence_gaps(spec_dir, orders[0], vectors)
        ordered = (pairs, exempt, pinned, shared)
        for gap in gaps:
            report("precedence", gap)
    if problems:
        print("check-vectors: %d problem(s)" % problems, file=sys.stderr)
        return 1
    print(
        "check-vectors: %d vector(s) in %d file(s) pass the checks against the spec: rule IDs, errors, "
        "events, chain restrictions, attestations and per-chain expectations" % (len(vectors), len(files))
    )
    if covered is not None:
        rules, retired = spec[0], spec[1]
        print(
            "coverage: each of %d MANDATE rule(s) %s has a passing and a failing vector; "
            "%d retired rule(s) and %d CHAINOPS rule(s) excluded"
            % (
                len(covered),
                "in %s" % ", ".join(areas) if areas is not None else "in the spec",
                len(retired & set(rules)),
                sum(1 for r in rules if r.startswith("CHAINOPS-")),
            )
        )
    if cases is not None:
        exempt = sorted(c for c in cases if c in PARITY_EXEMPT)
        print(
            "parity: %d of %d case(s) in %s have a %s vector; %d exempt: %s"
            % (len(cases) - len(exempt), len(cases), fixture, PARITY_PREFIX + "<case>", len(exempt), ", ".join(exempt) or "none")
        )
    if ordered is not None:
        pairs, exempt, pinned, shared = ordered
        print(
            "precedence: %d of %d adjacent pair(s) in %d check order(s) have a precedence vector; %d exempt: %s"
            % (pairs - len(exempt), pairs, sum(1 for o in orders[0].values() if o), len(exempt), ", ".join(exempt) or "none")
        )
        print(
            "precedence: in %d of the %d pinned pair(s) both checks return the error the pin expects, "
            "so the vector shows that error, not which check produced it" % (shared, pinned)
        )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
