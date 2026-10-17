// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs
pragma solidity 0.8.37;

import {BatchLimitsMandate} from "./BatchLimitsMandate.sol";
import {BatchLimitsToken} from "./BatchLimitsToken.sol";
import {Test, console} from "forge-std/Test.sol";

/// Measures batch request and batch execute gas on the prototype, per batch
/// size, leg count, recipient mode and token decimals. Run with
/// `forge test --match-contract BatchLimits -vv` to see the table.
contract BatchLimits is Test {
    uint256 internal constant ATTESTER_PK = 0xA77E57;
    address internal constant AGENT = address(0xA6E47);
    address internal constant PRINCIPAL = address(0x9819C1);
    address internal constant FEE_ADDRESS = address(0xFEE5);
    uint256 internal constant NOW = 1_800_000_000;
    uint256 internal constant HALF_ORDER = 0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0;

    /// Celo mainnet and Celo Sepolia block gas limit (eth_getBlockByNumber,
    /// 2026-10-17). Prague has no separate per-transaction cap.
    uint256 internal constant BLOCK_GAS_LIMIT = 30_000_000;
    /// EIP-7825 caps a transaction at 2^24 gas from Osaka on. Celo is not on
    /// Osaka yet, but a batch limit frozen now should survive it.
    uint256 internal constant OSAKA_TX_GAS_CAP = 16_777_216;
    uint256 internal constant HEADROOM_PERCENT = 30;

    /// The candidate limits under review, identical on both chains.
    uint256 internal constant MAX_BATCH = 10;
    uint256 internal constant MAX_BATCH_TRANSFERS = 18;

    uint256 internal seed;

    struct Gas {
        uint256 exec;
        uint256 calldataBytes;
        uint256 tx;
    }

    function setUp() public {
        vm.warp(NOW);
    }

    function _deploy(uint32 mode, BatchLimitsToken token) internal returns (BatchLimitsMandate m) {
        BatchLimitsMandate.Config memory c = BatchLimitsMandate.Config({
            principal: PRINCIPAL,
            asset: address(token),
            feeAddress: FEE_ADDRESS,
            attester: vm.addr(ATTESTER_PK),
            countryPolicyHash: bytes32(uint256(3)),
            recipientMode: mode,
            maxFeeBps: 100,
            payoutTtlSeconds: 86_400,
            // NOW is a fixed 2027 timestamp, far inside u64.
            // forge-lint: disable-next-line(unsafe-typecast)
            expiresAt: uint64(NOW + 30 days),
            totalBudget: type(uint128).max / 4,
            perPaymentCap: type(uint128).max / 4,
            perRecipientDailyCap: type(uint128).max / 4,
            // Above every request, so every item is approved and executable.
            approvalThreshold: type(uint128).max / 4
        });
        address[] memory agents = new address[](1);
        agents[0] = AGENT;
        m = new BatchLimitsMandate(c, agents);
        token.mint(address(m), type(uint128).max / 8);
    }

    function _attest(BatchLimitsMandate m, bytes32 ref, BatchLimitsMandate.Leg memory leg, uint64 validUntil)
        internal
        view
        returns (BatchLimitsMandate.Attestation memory)
    {
        (uint8 v, bytes32 r, bytes32 s) =
            vm.sign(ATTESTER_PK, m.digest(ref, leg.destination, leg.hasTag, leg.tag, validUntil));
        require(uint256(s) <= HALF_ORDER, "high s");
        return BatchLimitsMandate.Attestation(validUntil, abi.encodePacked(r, s, v));
    }

    /// A fresh destination per leg: the first credit to a zero balance is the
    /// expensive transfer. A tagged leg carries a full 64-bit tag, as a muxed
    /// Stellar destination would, so calldata, storage and events match the
    /// tagged case on Soroban.
    function _leg(uint256 i, uint256 j, uint256 unit, bool tagged)
        internal
        view
        returns (BatchLimitsMandate.Leg memory)
    {
        address dest = address(uint160(uint256(keccak256(abi.encode("dest", seed, i, j)))));
        // forge-lint: disable-next-line(unsafe-typecast)
        uint64 tag = tagged ? type(uint64).max - uint64(i * 8 + j) : 0;
        return BatchLimitsMandate.Leg({destination: dest, hasTag: tagged, tag: tag, amount: unit});
    }

    function _build(BatchLimitsMandate m, uint32 mode, uint256 items, uint256 legs, bool fee, uint256 unit)
        internal
        returns (BatchLimitsMandate.PayoutRequest[] memory)
    {
        (uint256[] memory legsPer, bool[] memory feePer) = _uniform(items, legs, fee);
        return _buildPlan(m, mode, legsPer, feePer, unit, false);
    }

    function _uniform(uint256 items, uint256 legs, bool fee)
        internal
        pure
        returns (uint256[] memory legsPer, bool[] memory feePer)
    {
        legsPer = new uint256[](items);
        feePer = new bool[](items);
        for (uint256 i = 0; i < items; ++i) {
            legsPer[i] = legs;
            feePer[i] = fee;
        }
    }

    /// One request per plan entry, with legsPer[i] legs and a fee if feePer[i].
    function _buildPlan(
        BatchLimitsMandate m,
        uint32 mode,
        uint256[] memory legsPer,
        bool[] memory feePer,
        uint256 unit,
        bool tagged
    ) internal returns (BatchLimitsMandate.PayoutRequest[] memory reqs) {
        reqs = new BatchLimitsMandate.PayoutRequest[](legsPer.length);
        uint256 allLegs;
        for (uint256 i = 0; i < reqs.length; ++i) {
            reqs[i] = _one(m, mode, i, legsPer[i], feePer[i], unit, tagged);
            allLegs += legsPer[i];
        }
        if (mode != m.MODE_ATTESTED()) _allowAll(m, reqs, allLegs);
        ++seed;
    }

    function _one(BatchLimitsMandate m, uint32 mode, uint256 i, uint256 legs, bool fee, uint256 unit, bool tagged)
        internal
        view
        returns (BatchLimitsMandate.PayoutRequest memory)
    {
        bytes32 ref = keccak256(abi.encode("ref", seed, i));
        BatchLimitsMandate.Leg[] memory ls = new BatchLimitsMandate.Leg[](legs);
        BatchLimitsMandate.Attestation[] memory atts =
            new BatchLimitsMandate.Attestation[](mode == m.MODE_ALLOWLIST() ? 0 : legs);
        for (uint256 j = 0; j < legs; ++j) {
            ls[j] = _leg(i, j, unit, tagged);
            // forge-lint: disable-next-line(unsafe-typecast)
            if (mode != m.MODE_ALLOWLIST()) atts[j] = _attest(m, ref, ls[j], uint64(NOW + 3600));
        }
        return BatchLimitsMandate.PayoutRequest({
            payoutId: keccak256(abi.encode("id", seed, i)),
            recipientRef: ref,
            total: unit * legs,
            fee: fee ? (unit * legs) / 100 : 0,
            legs: ls,
            // forge-lint: disable-next-line(unsafe-typecast)
            deadline: uint64(NOW + 600),
            taskRef: keccak256(abi.encode("task", seed, i)),
            attestations: atts
        });
    }

    function _allowAll(BatchLimitsMandate m, BatchLimitsMandate.PayoutRequest[] memory reqs, uint256 allLegs) internal {
        bytes32[] memory refs = new bytes32[](allLegs);
        BatchLimitsMandate.Leg[] memory allow = new BatchLimitsMandate.Leg[](allLegs);
        uint256 k;
        for (uint256 i = 0; i < reqs.length; ++i) {
            for (uint256 j = 0; j < reqs[i].legs.length; ++j) {
                refs[k] = reqs[i].recipientRef;
                allow[k++] = reqs[i].legs[j];
            }
        }
        vm.prank(PRINCIPAL);
        m.allowRecipients(refs, allow);
    }

    /// Gas a transaction carrying `data` needs: intrinsic plus calldata plus
    /// execution, or the EIP-7623 calldata floor (live since Prague) if higher.
    function _txGas(bytes memory data, uint256 exec) internal pure returns (Gas memory g) {
        uint256 zeros;
        for (uint256 i = 0; i < data.length; ++i) {
            if (data[i] == 0) ++zeros;
        }
        uint256 nonzeros = data.length - zeros;
        uint256 standard = 21_000 + zeros * 4 + nonzeros * 16 + exec;
        uint256 floor = 21_000 + 10 * (zeros + 4 * nonzeros);
        g = Gas({exec: exec, calldataBytes: data.length, tx: standard > floor ? standard : floor});
    }

    /// Runs one call as if it were its own transaction: every slot and
    /// account starts cold, as in a fresh transaction.
    /// Runs one call as if it were its own transaction: every slot and
    /// account starts cold. The call copies no return data, because growing
    /// this contract's memory to hold it would be charged inside the
    /// measurement, and that cost grows with everything the test allocated
    /// before. What remains on top of the callee's own gas is the call itself
    /// (2,600 for the cold account plus a few hundred), which a top-level
    /// transaction does not pay, so the figure is high by under 3,000.
    function _call(address target, address from, bytes memory data, address token) internal returns (Gas memory) {
        vm.cool(target);
        vm.cool(token);
        vm.prank(from);
        uint256 used;
        bool ok;
        assembly {
            let before := gas()
            ok := call(gas(), target, 0, add(data, 32), mload(data), 0, 0)
            used := sub(before, gas())
        }
        if (!ok) {
            assembly {
                let p := mload(0x40)
                returndatacopy(p, 0, returndatasize())
                revert(p, returndatasize())
            }
        }
        return _txGas(data, used);
    }

    function _measure(uint32 mode, uint256 legs, bool fee, uint8 decimals, uint256 items)
        internal
        returns (Gas memory, Gas memory)
    {
        (uint256[] memory legsPer, bool[] memory feePer) = _uniform(items, legs, fee);
        return _measurePlan(mode, legsPer, feePer, decimals, false);
    }

    /// Requests the planned batch, checks every item landed approved, then
    /// executes it and checks every item executed.
    function _measurePlan(uint32 mode, uint256[] memory legsPer, bool[] memory feePer, uint8 decimals, bool tagged)
        internal
        returns (Gas memory request, Gas memory execute)
    {
        BatchLimitsToken token = new BatchLimitsToken(decimals);
        BatchLimitsMandate m = _deploy(mode, token);
        BatchLimitsMandate.PayoutRequest[] memory reqs = _buildPlan(m, mode, legsPer, feePer, 10 ** decimals, tagged);
        request = _call(address(m), AGENT, abi.encodeCall(m.requestPayouts, (reqs)), address(token));
        bytes32[] memory ids = new bytes32[](reqs.length);
        for (uint256 i = 0; i < reqs.length; ++i) {
            ids[i] = reqs[i].payoutId;
            assertEq(m.statusOf(ids[i]), 2, "item not approved");
        }
        execute = _call(address(m), AGENT, abi.encodeCall(m.executeBatch, (ids)), address(token));
        for (uint256 i = 0; i < reqs.length; ++i) {
            assertEq(m.statusOf(ids[i]), 3, "item not executed");
        }
    }

    function _modeName(uint32 mode) internal pure returns (string memory) {
        return mode == 1 ? "allowlist" : mode == 2 ? "attested" : "both";
    }

    function _row(string memory op, uint32 mode, uint256 legs, bool fee, uint8 decimals, uint256 items, Gas memory g)
        internal
        pure
        returns (string memory)
    {
        Meta memory meta = Meta({
            mode: mode,
            legs: vm.toString(legs),
            fee: fee ? "yes" : "no",
            tagged: false,
            items: items,
            transfers: items * (legs + (fee ? 1 : 0))
        });
        return _rowL(op, meta, decimals, g);
    }

    /// The table line, with legs and fee as text so mixed plans can say what
    /// they hold.
    function _rowL(string memory op, Meta memory meta, uint8 decimals, Gas memory g)
        internal
        pure
        returns (string memory)
    {
        return string.concat(
            string.concat(
                "| ",
                op,
                " | ",
                _modeName(meta.mode),
                " | ",
                meta.legs,
                " | ",
                meta.fee,
                " | ",
                meta.tagged ? "yes" : "no",
                " | ",
                vm.toString(uint256(decimals)),
                " | ",
                vm.toString(meta.items),
                " | "
            ),
            string.concat(
                vm.toString(g.exec),
                " | ",
                vm.toString(g.calldataBytes),
                " | ",
                vm.toString(g.tx),
                " | ",
                _pct(g.tx, BLOCK_GAS_LIMIT),
                " | ",
                _pct(g.tx, OSAKA_TX_GAS_CAP),
                " |"
            )
        );
    }

    function _pct(uint256 part, uint256 whole) internal pure returns (string memory) {
        uint256 tenths = (part * 1000) / whole;
        return string.concat(vm.toString(tenths / 10), ".", vm.toString(tenths % 10), "%");
    }

    /// Largest batch whose transaction stays at or under 70 percent of
    /// `limit`, from a straight line through the 1 and 40 item points.
    function _maxItems(Gas memory one, Gas memory forty, uint256 limit) internal pure returns (uint256) {
        uint256 perItem = (forty.tx - one.tx) / 39;
        uint256 fixedPart = one.tx - perItem;
        uint256 budget = (limit * (100 - HEADROOM_PERCENT)) / 100;
        return (budget - fixedPart) / perItem;
    }

    function _header() internal pure {
        console.log(
            "| op | mode | legs | fee | tags | decimals | items | execution gas | calldata bytes | tx gas | share of 30M | share of 2^24 |"
        );
        console.log("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    }

    /// Measures one series at 1, 10, 20 and 40 items, logs every row, and
    /// returns the summary line with the largest safe batch.
    function _series(uint32 mode, uint256 legs, uint8 decimals, bool tagged) internal returns (string memory) {
        uint256[4] memory sizes = [uint256(1), 10, 20, 40];
        Gas[4] memory rq;
        Gas[4] memory ex;
        for (uint256 si = 0; si < sizes.length; ++si) {
            (uint256[] memory legsPer, bool[] memory feePer) = _uniform(sizes[si], legs, true);
            (rq[si], ex[si]) = _measurePlan(mode, legsPer, feePer, decimals, tagged);
            Meta memory meta = _meta(mode, legsPer, feePer, tagged);
            meta.legs = vm.toString(legs);
            meta.fee = "yes";
            console.log(_rowL("requestPayouts", meta, decimals, rq[si]));
            console.log(_rowL("executeBatch", meta, decimals, ex[si]));
        }
        return string.concat(
            string.concat(
                "| ",
                _modeName(mode),
                " | ",
                vm.toString(legs),
                " | ",
                tagged ? "yes" : "no",
                " | ",
                vm.toString(uint256(decimals)),
                " | "
            ),
            string.concat(
                vm.toString(_maxItems(rq[0], rq[3], BLOCK_GAS_LIMIT)),
                " / ",
                vm.toString(_maxItems(rq[0], rq[3], OSAKA_TX_GAS_CAP)),
                " | ",
                vm.toString(_maxItems(ex[0], ex[3], BLOCK_GAS_LIMIT)),
                " / ",
                vm.toString(_maxItems(ex[0], ex[3], OSAKA_TX_GAS_CAP)),
                " |"
            )
        );
    }

    /// One test per token, because a full table needs more than the 2^30 gas
    /// forge gives a single test.
    function test_batch_table_6_decimals() public {
        _table(6);
    }

    function test_batch_table_18_decimals() public {
        _table(18);
    }

    function _table(uint8 decimals) internal {
        uint8[1] memory decs = [decimals];
        uint32[3] memory modes = [uint32(1), 2, 3];
        uint256[3] memory legCounts = [uint256(1), 2, 4];
        _header();
        string[] memory summary = new string[](12);
        uint256 k;
        for (uint256 d = 0; d < decs.length; ++d) {
            for (uint256 mi = 0; mi < modes.length; ++mi) {
                for (uint256 li = 0; li < legCounts.length; ++li) {
                    summary[k++] = _series(modes[mi], legCounts[li], decs[d], false);
                }
            }
            // Tagged legs in the heaviest mode, for parity with the tagged
            // Soroban series.
            for (uint256 li = 0; li < legCounts.length; ++li) {
                summary[k++] = _series(3, legCounts[li], decs[d], true);
            }
        }
        console.log("");
        console.log("Largest batch with 30 percent headroom, against 30M block gas / the 2^24 Osaka tx cap (fee on):");
        console.log("| mode | legs | tags | decimals | requestPayouts | executeBatch |");
        console.log("| --- | --- | --- | --- | --- | --- |");
        for (uint256 i = 0; i < k; ++i) {
            console.log(summary[i]);
        }
    }

    function test_one_leg_without_fee() public {
        uint256[4] memory sizes = [uint256(1), 10, 20, 40];
        for (uint256 si = 0; si < sizes.length; ++si) {
            (Gas memory rq, Gas memory ex) = _measure(1, 1, false, 6, sizes[si]);
            console.log(_row("requestPayouts", 1, 1, false, 6, sizes[si], rq));
            console.log(_row("executeBatch", 1, 1, false, 6, sizes[si], ex));
        }
    }

    /// Every case here is measured: the candidate limits must leave 30
    /// percent headroom against the tighter of today's block limit and the
    /// Osaka transaction cap.
    function test_recommended_limits_keep_headroom() public {
        _header();
        // The largest request: every item at four tagged legs with a fee, in
        // the heaviest mode.
        (uint256[] memory legsPer, bool[] memory feePer) = _uniform(MAX_BATCH, 4, true);
        _checkPlan(3, legsPer, feePer, true, false);
        _checkPlan(3, legsPer, feePer, false, false);
        // Executes at exactly MAX_BATCH_TRANSFERS: every transfer a leg on as
        // many items as allowed, every item a 1-leg payout with a fee, and
        // four-leg payouts with and without a fee.
        (legsPer, feePer) = _worstPlan();
        _checkPlan(3, legsPer, feePer, true, true);
        (legsPer, feePer) = _uniform(MAX_BATCH_TRANSFERS / 2, 1, true);
        _checkPlan(3, legsPer, feePer, true, true);
        legsPer = new uint256[](4);
        feePer = new bool[](4);
        for (uint256 i = 0; i < 4; ++i) {
            legsPer[i] = 4;
            feePer[i] = i < 2;
        }
        _checkPlan(3, legsPer, feePer, true, true);
    }

    /// MAX_BATCH_TRANSFERS legs spread over MAX_BATCH items, no fee.
    function _worstPlan() internal pure returns (uint256[] memory legsPer, bool[] memory feePer) {
        legsPer = new uint256[](MAX_BATCH);
        feePer = new bool[](MAX_BATCH);
        for (uint256 i = 0; i < MAX_BATCH; ++i) {
            legsPer[i] = MAX_BATCH_TRANSFERS / MAX_BATCH + (i < MAX_BATCH_TRANSFERS % MAX_BATCH ? 1 : 0);
        }
    }

    /// How a measured plan is described in the table.
    struct Meta {
        uint32 mode;
        string legs;
        string fee;
        bool tagged;
        uint256 items;
        uint256 transfers;
    }

    function _meta(uint32 mode, uint256[] memory legsPer, bool[] memory feePer, bool tagged)
        internal
        pure
        returns (Meta memory meta)
    {
        uint256 legs;
        uint256 fees;
        for (uint256 i = 0; i < legsPer.length; ++i) {
            legs += legsPer[i];
            if (feePer[i]) ++fees;
        }
        meta = Meta({
            mode: mode,
            legs: string.concat(vm.toString(legs), " in all"),
            fee: string.concat(vm.toString(fees), " of ", vm.toString(legsPer.length)),
            tagged: tagged,
            items: legsPer.length,
            transfers: legs + fees
        });
    }

    /// Measures the plan and asserts both calls, or only the request when its
    /// execute would exceed the transfer budget. With `atBudget`, the plan
    /// must use the transfer budget exactly, so the limit itself is what is
    /// measured.
    function _checkPlan(uint32 mode, uint256[] memory legsPer, bool[] memory feePer, bool tagged, bool atBudget)
        internal
    {
        Meta memory meta = _meta(mode, legsPer, feePer, tagged);
        assertLe(meta.items, MAX_BATCH);
        if (atBudget) assertEq(meta.transfers, MAX_BATCH_TRANSFERS, "plan does not use the transfer budget exactly");
        (Gas memory rq, Gas memory ex) = _measurePlan(mode, legsPer, feePer, 18, tagged);
        _assertHeadroom("requestPayouts", meta, rq);
        if (meta.transfers <= MAX_BATCH_TRANSFERS) _assertHeadroom("executeBatch", meta, ex);
    }

    function _assertHeadroom(string memory op, Meta memory meta, Gas memory g) internal pure {
        console.log(_rowL(op, meta, 18, g));
        assertLe(g.tx, (OSAKA_TX_GAS_CAP * (100 - HEADROOM_PERCENT)) / 100);
    }
}
