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
    uint256 internal constant MAX_BATCH_TRANSFERS = 20;

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

    function _attest(BatchLimitsMandate m, bytes32 ref, address dest, uint64 validUntil)
        internal
        view
        returns (BatchLimitsMandate.Attestation memory)
    {
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(ATTESTER_PK, m.digest(ref, dest, false, 0, validUntil));
        require(uint256(s) <= HALF_ORDER, "high s");
        return BatchLimitsMandate.Attestation(validUntil, abi.encodePacked(r, s, v));
    }

    function _build(BatchLimitsMandate m, uint32 mode, uint256 items, uint256 legs, bool fee, uint256 unit)
        internal
        returns (BatchLimitsMandate.PayoutRequest[] memory reqs)
    {
        reqs = new BatchLimitsMandate.PayoutRequest[](items);
        bytes32[] memory refs = new bytes32[](items * legs);
        BatchLimitsMandate.Leg[] memory allow = new BatchLimitsMandate.Leg[](items * legs);
        for (uint256 i = 0; i < items; ++i) {
            bytes32 ref = keccak256(abi.encode("ref", seed, i));
            BatchLimitsMandate.Leg[] memory ls = new BatchLimitsMandate.Leg[](legs);
            BatchLimitsMandate.Attestation[] memory atts =
                new BatchLimitsMandate.Attestation[](mode == m.MODE_ALLOWLIST() ? 0 : legs);
            for (uint256 j = 0; j < legs; ++j) {
                // A fresh destination per leg: the first credit to a zero
                // balance is the expensive transfer.
                address dest = address(uint160(uint256(keccak256(abi.encode("dest", seed, i, j)))));
                ls[j] = BatchLimitsMandate.Leg(dest, false, 0, unit);
                refs[i * legs + j] = ref;
                allow[i * legs + j] = ls[j];
                // forge-lint: disable-next-line(unsafe-typecast)
                if (mode != m.MODE_ALLOWLIST()) atts[j] = _attest(m, ref, dest, uint64(NOW + 3600));
            }
            uint256 total = unit * legs;
            reqs[i] = BatchLimitsMandate.PayoutRequest({
                payoutId: keccak256(abi.encode("id", seed, i)),
                recipientRef: ref,
                total: total,
                fee: fee ? total / 100 : 0,
                legs: ls,
                // forge-lint: disable-next-line(unsafe-typecast)
                deadline: uint64(NOW + 600),
                taskRef: keccak256(abi.encode("task", seed, i)),
                attestations: atts
            });
        }
        if (mode != m.MODE_ATTESTED()) {
            vm.prank(PRINCIPAL);
            m.allowRecipients(refs, allow);
        }
        ++seed;
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
        returns (Gas memory request, Gas memory execute)
    {
        BatchLimitsToken token = new BatchLimitsToken(decimals);
        BatchLimitsMandate m = _deploy(mode, token);
        BatchLimitsMandate.PayoutRequest[] memory reqs = _build(m, mode, items, legs, fee, 10 ** decimals);
        request = _call(address(m), AGENT, abi.encodeCall(m.requestPayouts, (reqs)), address(token));
        bytes32[] memory ids = new bytes32[](items);
        for (uint256 i = 0; i < items; ++i) {
            ids[i] = reqs[i].payoutId;
            assertEq(m.statusOf(ids[i]), 2, "item not approved");
        }
        execute = _call(address(m), AGENT, abi.encodeCall(m.executeBatch, (ids)), address(token));
        for (uint256 i = 0; i < items; ++i) {
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
        return string.concat(
            string.concat(
                "| ",
                op,
                " | ",
                _modeName(mode),
                " | ",
                vm.toString(legs),
                " | ",
                fee ? "yes" : "no",
                " | ",
                vm.toString(uint256(decimals)),
                " | ",
                vm.toString(items),
                " | "
            ),
            string.concat(
                vm.toString(g.exec),
                " | ",
                vm.toString(g.calldataBytes),
                " | ",
                vm.toString(g.tx),
                " | ",
                vm.toString((g.tx * 1000) / BLOCK_GAS_LIMIT / 10),
                ".",
                vm.toString((g.tx * 1000) / BLOCK_GAS_LIMIT % 10),
                "% |"
            )
        );
    }

    /// Largest batch whose transaction stays at or under 70 percent of
    /// `limit`, from a straight line through the 1 and 40 item points.
    function _maxItems(Gas memory one, Gas memory forty, uint256 limit) internal pure returns (uint256) {
        uint256 perItem = (forty.tx - one.tx) / 39;
        uint256 fixedPart = one.tx - perItem;
        uint256 budget = (limit * (100 - HEADROOM_PERCENT)) / 100;
        return (budget - fixedPart) / perItem;
    }

    /// Measures one series at 1, 10, 20 and 40 items, logs every row, and
    /// returns the summary line with the largest safe batch.
    function _series(uint32 mode, uint256 legs, uint8 decimals) internal returns (string memory) {
        uint256[4] memory sizes = [uint256(1), 10, 20, 40];
        Gas[4] memory rq;
        Gas[4] memory ex;
        for (uint256 si = 0; si < sizes.length; ++si) {
            (rq[si], ex[si]) = _measure(mode, legs, true, decimals, sizes[si]);
            console.log(_row("requestPayouts", mode, legs, true, decimals, sizes[si], rq[si]));
            console.log(_row("executeBatch", mode, legs, true, decimals, sizes[si], ex[si]));
        }
        return string.concat(
            string.concat(
                "| ", _modeName(mode), " | ", vm.toString(legs), " | ", vm.toString(uint256(decimals)), " | "
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
        console.log(
            "| op | mode | legs | fee | decimals | items | execution gas | calldata bytes | tx gas | share of 30M |"
        );
        console.log("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |");
        string[] memory summary = new string[](9);
        uint256 k;
        for (uint256 d = 0; d < decs.length; ++d) {
            for (uint256 mi = 0; mi < modes.length; ++mi) {
                for (uint256 li = 0; li < legCounts.length; ++li) {
                    summary[k++] = _series(modes[mi], legCounts[li], decs[d]);
                }
            }
        }
        console.log("");
        console.log("Largest batch with 30 percent headroom, against 30M block gas / the 2^24 Osaka tx cap (fee on):");
        console.log("| mode | legs | decimals | requestPayouts | executeBatch |");
        console.log("| --- | --- | --- | --- | --- |");
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

    /// The candidate limits must leave 30 percent headroom against the
    /// tighter of today's block limit and the Osaka transaction cap.
    function test_recommended_limits_keep_headroom() public {
        uint256 budget = (OSAKA_TX_GAS_CAP * (100 - HEADROOM_PERCENT)) / 100;
        // (mode, legs, fee, items), with items * (legs + fee) <= MAX_BATCH_TRANSFERS on execute.
        (Gas memory rq, Gas memory ex) = _measure(3, 4, true, 18, 4);
        assertLe(rq.tx, budget);
        assertLe(ex.tx, budget);
        (rq, ex) = _measure(3, 2, false, 18, MAX_BATCH);
        assertLe(rq.tx, budget);
        assertLe(ex.tx, budget);
        (rq,) = _measure(3, 4, true, 18, MAX_BATCH);
        console.log(_row("requestPayouts", 3, 4, true, 18, MAX_BATCH, rq));
        assertLe(rq.tx, budget);
        (rq, ex) = _measure(1, 1, true, 18, MAX_BATCH);
        assertLe(rq.tx, budget);
        assertLe(ex.tx, budget);
        assertLe(MAX_BATCH * 2, MAX_BATCH_TRANSFERS);
    }
}
