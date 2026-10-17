// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs
pragma solidity 0.8.37;

interface IBatchToken {
    function transfer(address to, uint256 amount) external returns (bool);
    function balanceOf(address account) external view returns (uint256);
}

/// Spike only, kept under test/ so it never ships. A prototype of the
/// mandate's batch request and batch execute with the real storage layout,
/// check order, events and transfers, built to measure what a batch costs.
/// Everything a batch does not touch (approvals, cancel, pause, most roles)
/// is left out.
contract BatchLimitsMandate {
    struct Leg {
        address destination;
        bool hasTag;
        uint64 tag;
        uint256 amount;
    }

    struct Attestation {
        uint64 validUntil;
        bytes signature;
    }

    struct PayoutRequest {
        bytes32 payoutId;
        bytes32 recipientRef;
        uint256 total;
        uint256 fee;
        Leg[] legs;
        uint64 deadline;
        bytes32 taskRef;
        Attestation[] attestations;
    }

    struct ItemResult {
        bytes32 payoutId;
        uint32 code;
        uint32 status;
    }

    /// Amounts never exceed I128_MAX, so they are stored as uint128.
    struct StoredLeg {
        address destination;
        bool hasTag;
        uint64 tag;
        uint128 amount;
    }

    struct Payout {
        address agent;
        uint32 status;
        uint64 createdAt;
        uint64 expiresAt;
        uint32 policyEpoch;
        bytes32 recipientRef;
        uint128 total;
        uint128 fee;
        StoredLeg[] legs;
    }

    struct Daily {
        uint64 day;
        uint128 used;
    }

    struct Config {
        address principal;
        address asset;
        address feeAddress;
        address attester;
        bytes32 countryPolicyHash;
        uint32 recipientMode;
        uint32 maxFeeBps;
        uint64 payoutTtlSeconds;
        uint64 expiresAt;
        uint128 totalBudget;
        uint128 perPaymentCap;
        uint128 perRecipientDailyCap;
        uint128 approvalThreshold;
    }

    error MandateError(uint32 code);

    event PayoutRequested(
        bytes32 indexed payoutId,
        address agent,
        bytes32 recipientRef,
        uint256 total,
        uint256 fee,
        Leg[] legs,
        bytes32 taskRef,
        uint32 status,
        uint64 expiresAt
    );
    event PayoutRequestRejected(bytes32 indexed payoutId, address agent, uint32 code);
    event PayoutExecuted(
        bytes32 indexed payoutId,
        bytes32 recipientRef,
        uint256 total,
        uint256 fee,
        Leg[] legs,
        uint64 day,
        uint256 spent
    );

    uint32 internal constant UNAUTHORIZED = 10;
    uint32 internal constant MANDATE_EXPIRED = 21;
    uint32 internal constant AMOUNT_INVALID = 30;
    uint32 internal constant LEGS_INVALID = 31;
    uint32 internal constant LEGS_SUM_MISMATCH = 32;
    uint32 internal constant BATCH_INVALID = 33;
    uint32 internal constant REQUEST_DEADLINE_PASSED = 35;
    uint32 internal constant BUDGET_EXCEEDED = 40;
    uint32 internal constant PAYMENT_CAP_EXCEEDED = 41;
    uint32 internal constant DAILY_CAP_EXCEEDED = 42;
    uint32 internal constant INSUFFICIENT_FUNDS = 43;
    uint32 internal constant FEE_EXCEEDS_CAP = 46;
    uint32 internal constant RECIPIENT_NOT_ALLOWED = 50;
    uint32 internal constant ATTESTATION_INVALID = 51;
    uint32 internal constant ATTESTATION_EXPIRED = 52;
    uint32 internal constant POLICY_CHANGED = 53;
    uint32 internal constant PAYOUT_ID_EXISTS = 60;
    uint32 internal constant PAYOUT_NOT_FOUND = 61;
    uint32 internal constant PAYOUT_WRONG_STATUS = 62;
    uint32 internal constant PAYOUT_LAPSED = 63;
    uint32 internal constant TRANSFER_FAILED = 72;

    uint32 public constant MODE_ALLOWLIST = 1;
    uint32 public constant MODE_ATTESTED = 2;
    uint32 public constant MODE_BOTH = 3;
    uint32 internal constant STATUS_PENDING = 1;
    uint32 internal constant STATUS_APPROVED = 2;
    uint32 internal constant STATUS_EXECUTED = 3;

    uint256 internal constant MAX_LEGS = 4;
    /// Above every candidate MAX_BATCH, so the spike can measure past it.
    uint256 public constant PROBE_MAX_BATCH = 64;
    uint256 internal constant I128_MAX = uint256(uint128(type(int128).max));
    uint256 internal constant SECONDS_PER_DAY = 86_400;
    uint256 internal constant BPS_DENOMINATOR = 10_000;
    uint256 internal constant MAX_ATTESTATION_VALIDITY = 86_400;
    uint256 internal constant ORDER = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141;
    uint256 internal constant HALF_ORDER = 0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0;
    bytes15 internal constant DOMAIN = "UJIRA/ATTEST/V1";

    Config internal cfg;
    mapping(address => bool) public isAgent;
    uint128 public spent;
    uint128 public pending;
    uint128 public committed;
    uint32 public policyEpoch;
    mapping(bytes32 => Payout) internal payouts;
    mapping(bytes32 => Daily) internal daily;
    mapping(bytes32 => bool) internal allowed;

    constructor(Config memory config, address[] memory agents) {
        cfg = config;
        for (uint256 i = 0; i < agents.length; ++i) {
            isAgent[agents[i]] = true;
        }
    }

    modifier nonReentrant() {
        assembly {
            if tload(0) { revert(0, 0) }
            tstore(0, 1)
        }
        _;
        assembly {
            tstore(0, 0)
        }
    }

    function allowRecipients(bytes32[] calldata refs, Leg[] calldata destinations) external {
        if (msg.sender != cfg.principal) revert MandateError(UNAUTHORIZED);
        for (uint256 i = 0; i < refs.length; ++i) {
            Leg calldata d = destinations[i];
            allowed[_allowKey(refs[i], d.destination, d.hasTag, d.tag)] = true;
        }
    }

    function statusOf(bytes32 payoutId) external view returns (uint32) {
        return payouts[payoutId].status;
    }

    function requestPayouts(PayoutRequest[] calldata reqs) external nonReentrant returns (ItemResult[] memory results) {
        if (!isAgent[msg.sender]) revert MandateError(UNAUTHORIZED);
        // Mandate expiry is a rule on block time by design.
        // slither-disable-next-line timestamp
        if (block.timestamp >= cfg.expiresAt) revert MandateError(MANDATE_EXPIRED);
        if (reqs.length == 0 || reqs.length > PROBE_MAX_BATCH) revert MandateError(BATCH_INVALID);
        // One balance read per call; items are judged against it plus what
        // earlier items in the batch reserved.
        uint256 balance = IBatchToken(cfg.asset).balanceOf(address(this));
        results = new ItemResult[](reqs.length);
        for (uint256 i = 0; i < reqs.length; ++i) {
            (uint32 code, uint32 status) = _request(reqs[i], balance);
            if (code != 0) emit PayoutRequestRejected(reqs[i].payoutId, msg.sender, code);
            results[i] = ItemResult({payoutId: reqs[i].payoutId, code: code, status: status});
        }
    }

    function executeBatch(bytes32[] calldata ids) external nonReentrant {
        if (msg.sender != cfg.principal && !isAgent[msg.sender]) revert MandateError(UNAUTHORIZED);
        // slither-disable-next-line timestamp
        if (block.timestamp >= cfg.expiresAt) revert MandateError(MANDATE_EXPIRED);
        if (ids.length == 0 || ids.length > PROBE_MAX_BATCH) revert MandateError(BATCH_INVALID);
        uint256 balance = IBatchToken(cfg.asset).balanceOf(address(this));
        for (uint256 i = 0; i < ids.length; ++i) {
            balance = _execute(ids[i], balance);
        }
    }

    function _execute(bytes32 id, uint256 balance) internal returns (uint256) {
        (Leg[] memory legs, bytes32 ref, uint256 total, uint256 fee, uint256 usedAfter) = _checkExecute(id, balance);
        uint256 charged = total + fee;
        // A day number fits u64 for any block time a chain can reach.
        // forge-lint: disable-next-line(unsafe-typecast)
        uint64 today = uint64(block.timestamp / SECONDS_PER_DAY);

        // Checks, effects, interactions. The record becomes a tombstone that
        // keeps only its status, so the ID can never be reused.
        delete payouts[id];
        payouts[id].status = STATUS_EXECUTED;
        // Every amount here was bounded by I128_MAX at request, and the daily
        // sum by the cap, so none of these casts can truncate.
        // forge-lint: disable-next-line(unsafe-typecast)
        committed -= uint128(charged);
        // forge-lint: disable-next-line(unsafe-typecast)
        spent += uint128(charged);
        // forge-lint: disable-next-line(unsafe-typecast)
        daily[ref] = Daily({day: today, used: uint128(usedAfter)});
        emit PayoutExecuted(id, ref, total, fee, legs, today, spent);

        address asset = cfg.asset;
        for (uint256 j = 0; j < legs.length; ++j) {
            _safeTransfer(asset, legs[j].destination, legs[j].amount);
        }
        if (fee > 0) _safeTransfer(asset, cfg.feeAddress, fee);
        return balance - charged;
    }

    /// The execute checks in order: found, status, lapse, per-payment cap,
    /// fee cap, recipient binding, daily cap, funds. Reverts with the first
    /// failing code.
    function _checkExecute(bytes32 id, uint256 balance)
        internal
        view
        returns (Leg[] memory legs, bytes32 ref, uint256 total, uint256 fee, uint256 usedAfter)
    {
        Payout storage p = payouts[id];
        uint32 status = p.status;
        if (status == 0) revert MandateError(PAYOUT_NOT_FOUND);
        if (status != STATUS_APPROVED) revert MandateError(PAYOUT_WRONG_STATUS);
        // slither-disable-next-line timestamp
        if (block.timestamp >= p.expiresAt) revert MandateError(PAYOUT_LAPSED);
        total = p.total;
        fee = p.fee;
        if (total + fee > cfg.perPaymentCap) revert MandateError(PAYMENT_CAP_EXCEEDED);
        if (fee > _feeCap(total, cfg.maxFeeBps)) revert MandateError(FEE_EXCEEDS_CAP);
        ref = p.recipientRef;
        uint32 mode = cfg.recipientMode;
        legs = new Leg[](p.legs.length);
        for (uint256 j = 0; j < legs.length; ++j) {
            StoredLeg storage s = p.legs[j];
            legs[j] = Leg({destination: s.destination, hasTag: s.hasTag, tag: s.tag, amount: s.amount});
            if (mode != MODE_ATTESTED && !allowed[_allowKey(ref, s.destination, s.hasTag, s.tag)]) {
                revert MandateError(RECIPIENT_NOT_ALLOWED);
            }
        }
        if (mode != MODE_ALLOWLIST && p.policyEpoch != policyEpoch) revert MandateError(POLICY_CHANGED);
        Daily storage d = daily[ref];
        usedAfter = (d.day == block.timestamp / SECONDS_PER_DAY ? d.used : 0) + total;
        if (usedAfter > cfg.perRecipientDailyCap) revert MandateError(DAILY_CAP_EXCEEDED);
        if (total + fee > balance) revert MandateError(INSUFFICIENT_FUNDS);
    }

    /// Every check of a single request in order, then its effects. Returns a code
    /// instead of reverting so one bad item never sinks the batch; nothing is
    /// written unless every check passes.
    function _request(PayoutRequest calldata req, uint256 balance) internal returns (uint32 code, uint32 status) {
        code = _checkShape(req);
        if (code == 0) code = _checkBinding(req);
        if (code == 0) code = _checkLimits(req, balance);
        if (code != 0) return (code, 0);
        return (0, _store(req));
    }

    /// The shape checks: ID unused, deadline, amounts, legs, leg sum.
    function _checkShape(PayoutRequest calldata req) internal view returns (uint32) {
        if (payouts[req.payoutId].status != 0) return PAYOUT_ID_EXISTS;
        // slither-disable-next-line timestamp
        if (block.timestamp >= req.deadline) return REQUEST_DEADLINE_PASSED;
        if (req.total < 1 || req.total > I128_MAX || req.fee > I128_MAX - req.total) return AMOUNT_INVALID;
        uint256 n = req.legs.length;
        if (n < 1 || n > MAX_LEGS) return LEGS_INVALID;
        uint256 sum;
        for (uint256 i = 0; i < n; ++i) {
            Leg calldata leg = req.legs[i];
            if (leg.amount < 1 || leg.amount > I128_MAX || leg.destination == address(0)) return LEGS_INVALID;
            if (!leg.hasTag && leg.tag != 0) return LEGS_INVALID;
            for (uint256 k = i + 1; k < n; ++k) {
                Leg calldata other = req.legs[k];
                if (other.destination == leg.destination && other.hasTag == leg.hasTag && other.tag == leg.tag) {
                    return LEGS_INVALID;
                }
            }
            sum += leg.amount;
        }
        if (sum != req.total) return LEGS_SUM_MISMATCH;
        return 0;
    }

    /// Recipient binding: allowlist, attestation, or both, leg by leg.
    function _checkBinding(PayoutRequest calldata req) internal view returns (uint32) {
        uint32 mode = cfg.recipientMode;
        uint256 n = req.legs.length;
        if (mode == MODE_ALLOWLIST) {
            if (req.attestations.length != 0) return ATTESTATION_INVALID;
        } else if (req.attestations.length != n) {
            return ATTESTATION_INVALID;
        }
        for (uint256 i = 0; i < n; ++i) {
            Leg calldata leg = req.legs[i];
            if (mode != MODE_ATTESTED && !allowed[_allowKey(req.recipientRef, leg.destination, leg.hasTag, leg.tag)]) {
                return RECIPIENT_NOT_ALLOWED;
            }
            if (mode != MODE_ALLOWLIST) {
                uint32 code = _verifyAttestation(req.recipientRef, leg, req.attestations[i]);
                if (code != 0) return code;
            }
        }
        return 0;
    }

    /// The limit checks: fee cap, per-payment cap, budget, daily cap, funds.
    function _checkLimits(PayoutRequest calldata req, uint256 balance) internal view returns (uint32) {
        uint256 total = req.total;
        uint256 fee = req.fee;
        if (fee > _feeCap(total, cfg.maxFeeBps)) return FEE_EXCEEDS_CAP;
        uint256 charged = total + fee;
        if (charged > cfg.perPaymentCap) return PAYMENT_CAP_EXCEEDED;
        if (charged > cfg.totalBudget - (uint256(spent) + pending + committed)) return BUDGET_EXCEEDED;
        Daily storage d = daily[req.recipientRef];
        uint256 used = d.day == block.timestamp / SECONDS_PER_DAY ? d.used : 0;
        if (total > cfg.perRecipientDailyCap - used) return DAILY_CAP_EXCEEDED;
        uint256 held = uint256(pending) + committed;
        if (charged > (balance > held ? balance - held : 0)) return INSUFFICIENT_FUNDS;
        return 0;
    }

    function _store(PayoutRequest calldata req) internal returns (uint32 status) {
        uint256 charged = req.total + req.fee;
        if (charged >= cfg.approvalThreshold) {
            status = STATUS_PENDING;
            // charged <= I128_MAX was checked in _checkShape.
            // forge-lint: disable-next-line(unsafe-typecast)
            pending += uint128(charged);
        } else {
            status = STATUS_APPROVED;
            // forge-lint: disable-next-line(unsafe-typecast)
            committed += uint128(charged);
        }
        uint64 expiresAt = uint64(block.timestamp) + cfg.payoutTtlSeconds;
        Payout storage p = payouts[req.payoutId];
        p.agent = msg.sender;
        p.status = status;
        p.createdAt = uint64(block.timestamp);
        p.expiresAt = expiresAt;
        p.policyEpoch = policyEpoch;
        p.recipientRef = req.recipientRef;
        p.total = uint128(req.total);
        p.fee = uint128(req.fee);
        for (uint256 i = 0; i < req.legs.length; ++i) {
            Leg calldata leg = req.legs[i];
            // Leg amounts were bounded by I128_MAX in _checkShape.
            // forge-lint: disable-next-line(unsafe-typecast)
            uint128 amount = uint128(leg.amount);
            p.legs.push(StoredLeg({destination: leg.destination, hasTag: leg.hasTag, tag: leg.tag, amount: amount}));
        }
        emit PayoutRequested(
            req.payoutId, msg.sender, req.recipientRef, req.total, req.fee, req.legs, req.taskRef, status, expiresAt
        );
    }

    function digest(bytes32 recipientRef, address destination, bool hasTag, uint64 tag, uint64 validUntil)
        public
        view
        returns (bytes32)
    {
        return keccak256(
            abi.encodePacked(
                DOMAIN,
                bytes32(block.chainid),
                recipientRef,
                cfg.countryPolicyHash,
                validUntil,
                uint8(20),
                address(this),
                uint8(20),
                destination,
                hasTag ? uint8(1) : uint8(0),
                tag
            )
        );
    }

    function _verifyAttestation(bytes32 ref, Leg calldata leg, Attestation calldata att)
        internal
        view
        returns (uint32)
    {
        // slither-disable-next-line timestamp
        if (block.timestamp >= att.validUntil) return ATTESTATION_EXPIRED;
        // slither-disable-next-line timestamp
        if (att.validUntil - block.timestamp > MAX_ATTESTATION_VALIDITY) return ATTESTATION_INVALID;
        bytes calldata sig = att.signature;
        if (sig.length != 65) return ATTESTATION_INVALID;
        uint8 v = uint8(sig[64]);
        if (v != 27 && v != 28) return ATTESTATION_INVALID;
        bytes32 r = bytes32(sig[0:32]);
        bytes32 s = bytes32(sig[32:64]);
        if (uint256(r) == 0 || uint256(r) >= ORDER || uint256(s) == 0 || uint256(s) > HALF_ORDER) {
            return ATTESTATION_INVALID;
        }
        address recovered = ecrecover(digest(ref, leg.destination, leg.hasTag, leg.tag, att.validUntil), v, r, s);
        // An unrecoverable signature aborts the whole call, as it does on Soroban.
        if (recovered == address(0)) revert MandateError(ATTESTATION_INVALID);
        if (recovered != cfg.attester) return ATTESTATION_INVALID;
        return 0;
    }

    /// floor(total * bps / 10000) without forming total * bps.
    function _feeCap(uint256 total, uint256 bps) internal pure returns (uint256) {
        // Dividing first is the point: the remainder term restores the
        // precision, and the product can never overflow.
        // forge-lint: disable-next-line(divide-before-multiply)
        return (total / BPS_DENOMINATOR) * bps + ((total % BPS_DENOMINATOR) * bps) / BPS_DENOMINATOR;
    }

    function _allowKey(bytes32 ref, address destination, bool hasTag, uint64 tag) internal pure returns (bytes32) {
        return keccak256(abi.encode(ref, destination, hasTag, tag));
    }

    /// A revert, a false return or missing return data all count as a failed
    /// transfer.
    function _safeTransfer(address token, address to, uint256 amount) internal {
        // slither-disable-next-line low-level-calls
        (bool ok, bytes memory ret) = token.call(abi.encodeCall(IBatchToken.transfer, (to, amount)));
        if (!ok || ret.length != 32 || abi.decode(ret, (uint256)) != 1) revert MandateError(TRANSFER_FAILED);
    }
}
