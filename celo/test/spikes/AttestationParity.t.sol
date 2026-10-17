// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs
pragma solidity 0.8.37;

import {AttestationParityVerifier} from "./AttestationParityVerifier.sol";
import {Test, Vm, console} from "forge-std/Test.sol";

/// Shared helpers for building legs, attestations and honest signatures.
abstract contract AttestationFixtureBase is Test {
    uint256 internal constant ORDER = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141;
    uint256 internal constant HALF_ORDER = 0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0;
    uint32 internal constant ATTESTATION_INVALID = 51;
    uint32 internal constant ATTESTATION_EXPIRED = 52;
    uint64 internal constant NOW = 1_767_225_600;

    /// Seconds after NOW, bounded so the sum always fits the attestation's
    /// uint64 field.
    function _after(uint256 dt, uint256 lo, uint256 hi) internal pure returns (uint64) {
        return NOW + _toUint64(bound(dt, lo, hi));
    }

    function _toUint64(uint256 x) internal pure returns (uint64) {
        require(x <= type(uint64).max, "does not fit uint64");
        // forge-lint: disable-next-line(unsafe-typecast)
        return uint64(x);
    }

    function _uncompressed(uint256 pk) internal returns (bytes memory) {
        Vm.Wallet memory w = vm.createWallet(pk);
        return abi.encodePacked(uint8(0x04), w.publicKeyX, w.publicKeyY);
    }

    function _sign(uint256 pk, bytes32 d) internal pure returns (bytes memory) {
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(pk, d);
        return abi.encodePacked(r, s, v);
    }

    function _one(address dest, bool hasTag, uint64 tag, uint64 validUntil, bytes memory sig)
        internal
        pure
        returns (AttestationParityVerifier.Leg[] memory legs, AttestationParityVerifier.Attestation[] memory atts)
    {
        legs = new AttestationParityVerifier.Leg[](1);
        atts = new AttestationParityVerifier.Attestation[](1);
        legs[0] = AttestationParityVerifier.Leg(dest, hasTag, tag, 500);
        atts[0] = AttestationParityVerifier.Attestation(validUntil, sig);
    }

    function _err(uint32 code) internal pure returns (bytes memory) {
        return abi.encodeWithSelector(AttestationParityVerifier.MandateError.selector, code);
    }
}

contract AttestationParityTest is AttestationFixtureBase {
    string internal constant FIXTURE = "../spikes/attestation-parity/fixture.json";
    string internal json;

    function setUp() public {
        // Read only, and foundry.toml allows this one file and nothing else.
        // forge-lint: disable-next-line(unsafe-cheatcode)
        json = vm.readFile(FIXTURE);
    }

    function _key(uint256 i, string memory path) internal pure returns (string memory) {
        return string.concat(".cases[", vm.toString(i), "]", path);
    }

    function _str(uint256 i, string memory path) internal view returns (string memory) {
        return vm.parseJsonString(json, _key(i, path));
    }

    function _uint(uint256 i, string memory path) internal view returns (uint256) {
        return vm.parseUint(_str(i, path));
    }

    function _count(uint256 i, string memory path) internal view returns (uint256 n) {
        while (vm.keyExistsJson(json, _key(i, string.concat(path, "[", vm.toString(n), "]")))) {
            ++n;
        }
    }

    function _leg(uint256 i, uint256 j) internal view returns (AttestationParityVerifier.Leg memory) {
        string memory p = string.concat(".celo.legs[", vm.toString(j), "]");
        return AttestationParityVerifier.Leg({
            destination: vm.parseJsonAddress(json, _key(i, string.concat(p, ".destination"))),
            hasTag: vm.parseJsonBool(json, _key(i, string.concat(p, ".has_tag"))),
            tag: uint64(_uint(i, string.concat(p, ".tag"))),
            amount: _uint(i, string.concat(p, ".amount"))
        });
    }

    function _att(uint256 i, uint256 j) internal view returns (AttestationParityVerifier.Attestation memory) {
        string memory p = string.concat(".celo.attestations[", vm.toString(j), "]");
        return AttestationParityVerifier.Attestation({
            validUntil: uint64(_uint(i, string.concat(p, ".valid_until"))),
            signature: vm.parseJsonBytes(json, _key(i, string.concat(p, ".signature")))
        });
    }

    /// Puts the verifier at the case's mandate address on the case's chain,
    /// so the digest is built from the same environment the fixture assumed.
    function _deploy(uint256 i) internal returns (AttestationParityVerifier) {
        vm.chainId(_uint(i, ".celo.chain_id"));
        assertEq(bytes32(block.chainid), vm.parseJsonBytes32(json, _key(i, ".celo.chain_tag")), "chain tag");
        vm.warp(_uint(i, ".now"));
        address mandate = vm.parseJsonAddress(json, _key(i, ".celo.mandate"));
        // Runs the constructor at the mandate address, so address(this) in the
        // digest is the mandate the fixture names.
        deployCodeTo(
            "AttestationParityVerifier.sol:AttestationParityVerifier",
            abi.encode(
                vm.parseJsonBytes(json, _key(i, ".attester_public_key")),
                vm.parseJsonBytes32(json, _key(i, ".country_policy_hash"))
            ),
            mandate
        );
        AttestationParityVerifier verifier = AttestationParityVerifier(mandate);
        assertEq(verifier.attester(), vm.parseJsonAddress(json, _key(i, ".attester_evm_address")), "attester");
        return verifier;
    }

    function test_FixtureKeysAreWhatTheyClaim() public {
        uint256 pk = vm.parseJsonUint(json, ".keys.attester.private_key");
        assertEq(pk, uint256(keccak256("UJIRA/TEST-ONLY/S18/attester")));
        assertEq(vm.addr(pk), vm.parseJsonAddress(json, ".keys.attester.evm_address"));
        assertEq(_uncompressed(pk), vm.parseJsonBytes(json, ".keys.attester.public_key"));
    }

    function test_EveryFixtureCaseMatches() public {
        uint256 cases;
        while (vm.keyExistsJson(json, string.concat(".cases[", vm.toString(cases), "]"))) {
            ++cases;
        }
        assertGe(cases, 40, "fixture lost cases");

        for (uint256 i = 0; i < cases; ++i) {
            string memory name = _str(i, ".name");
            AttestationParityVerifier verifier = _deploy(i);
            bytes32 ref = vm.parseJsonBytes32(json, _key(i, ".recipient_ref"));

            uint256 nLegs = _count(i, ".celo.legs");
            uint256 nAtts = _count(i, ".celo.attestations");
            AttestationParityVerifier.Leg[] memory legs = new AttestationParityVerifier.Leg[](nLegs);
            AttestationParityVerifier.Attestation[] memory atts = new AttestationParityVerifier.Attestation[](nAtts);
            for (uint256 j = 0; j < nLegs; ++j) {
                legs[j] = _leg(i, j);
            }
            for (uint256 j = 0; j < nAtts; ++j) {
                atts[j] = _att(i, j);
                if (j < nLegs) {
                    bytes32 want = vm.parseJsonBytes32(
                        json, _key(i, string.concat(".celo.attestations[", vm.toString(j), "].digest"))
                    );
                    bytes32 got =
                        verifier.digest(ref, legs[j].destination, legs[j].hasTag, legs[j].tag, atts[j].validUntil);
                    assertEq(got, want, string.concat(name, ": digest"));
                }
            }

            uint256 code = vm.parseJsonUint(json, _key(i, ".expect_code"));
            uint256 calls = vm.parseJsonUint(json, _key(i, ".calls"));
            for (uint256 c = 0; c < calls; ++c) {
                if (code == 0) {
                    verifier.verify(ref, legs, atts);
                } else {
                    require(code <= type(uint32).max, "code does not fit uint32");
                    // forge-lint: disable-next-line(unsafe-typecast)
                    vm.expectRevert(_err(uint32(code)));
                    verifier.verify(ref, legs, atts);
                }
            }
            console.log(name, _str(i, ".expect"));
        }
    }

    /// Gas for one verification, and the marginal cost of each extra leg.
    function test_GasPerLeg() public {
        uint256 one = _caseIndex("valid_untagged");
        uint256 two = _caseIndex("valid_two_legs_same_address_two_tags");

        uint256 g1 = _measure(one);
        uint256 g2 = _measure(two);
        console.log("celo verify gas, 1 leg (account and storage warm):", g1);
        console.log("celo verify gas, 2 legs (account and storage warm):", g2);
        console.log("celo marginal gas per leg:", g2 - g1);
        assertGt(g2, g1);
    }

    function _measure(uint256 i) internal returns (uint256) {
        AttestationParityVerifier verifier = _deploy(i);
        bytes32 ref = vm.parseJsonBytes32(json, _key(i, ".recipient_ref"));
        uint256 n = _count(i, ".celo.legs");
        AttestationParityVerifier.Leg[] memory legs = new AttestationParityVerifier.Leg[](n);
        AttestationParityVerifier.Attestation[] memory atts = new AttestationParityVerifier.Attestation[](n);
        for (uint256 j = 0; j < n; ++j) {
            legs[j] = _leg(i, j);
            atts[j] = _att(i, j);
        }
        // Warm the account first: in a real request the mandate is the call
        // target, so it is never cold.
        verifier.verify(ref, legs, atts);
        uint256 before = gasleft();
        verifier.verify(ref, legs, atts);
        return before - gasleft();
    }

    function _caseIndex(string memory name) internal view returns (uint256 i) {
        while (keccak256(bytes(_str(i, ".name"))) != keccak256(bytes(name))) {
            ++i;
        }
    }
}

/// Celo-only edges and properties that hold for any key and input, not just
/// the fixture's.
contract AttestationParityPropertiesTest is AttestationFixtureBase {
    bytes32 internal constant POLICY = keccak256("policy");
    uint256 internal constant PK = uint256(keccak256("UJIRA/TEST-ONLY/S18/fuzz-attester"));
    AttestationParityVerifier internal verifier;

    function setUp() public {
        vm.warp(NOW);
        verifier = new AttestationParityVerifier(_uncompressed(PK), POLICY);
    }

    function test_ScalarRangeBoundaries() public view {
        assertTrue(verifier.scalarsInRange(1, HALF_ORDER), "s = n/2 is low");
        assertFalse(verifier.scalarsInRange(1, HALF_ORDER + 1), "s = n/2 + 1 is high");
        assertTrue(verifier.scalarsInRange(ORDER - 1, 1), "r = n - 1");
        assertFalse(verifier.scalarsInRange(ORDER, 1), "r = n");
        assertFalse(verifier.scalarsInRange(0, 1), "r = 0");
        assertFalse(verifier.scalarsInRange(1, 0), "s = 0");
    }

    function test_ConstructorEmitsPolicy() public {
        vm.expectEmit(false, false, false, true);
        emit AttestationParityVerifier.AttestationPolicySet(vm.addr(PK), POLICY);
        new AttestationParityVerifier(_uncompressed(PK), POLICY);
    }

    function test_ConstructorRejectsCompressedKey() public {
        bytes memory key = _uncompressed(PK);
        key[0] = 0x02;
        vm.expectRevert(_err(34));
        new AttestationParityVerifier(key, POLICY);
    }

    function test_ConstructorRejectsShortKey() public {
        vm.expectRevert(_err(34));
        new AttestationParityVerifier(hex"04", POLICY);
    }

    /// The mandate rejects hasTag = false with tag != 0 earlier, as
    /// LEGS_INVALID. Even if that check were missing, the digest would not
    /// match what any signer produced for the untagged destination.
    function test_NonCanonicalTagNeverMatchesUntaggedSignature() public {
        address dest = makeAddr("dest");
        uint64 vu = NOW + 3600;
        bytes memory sig = _sign(PK, verifier.digest(bytes32(uint256(1)), dest, false, 0, vu));
        (AttestationParityVerifier.Leg[] memory legs, AttestationParityVerifier.Attestation[] memory atts) =
            _one(dest, false, 7, vu, sig);
        vm.expectRevert(_err(ATTESTATION_INVALID));
        verifier.verify(bytes32(uint256(1)), legs, atts);
    }

    function testFuzz_AcceptsAnyHonestSignature(bytes32 ref, address dest, bool hasTag, uint64 tag, uint32 dt)
        public
        view
    {
        if (!hasTag) tag = 0;
        uint64 vu = _after(dt, 1, 86_400);
        bytes memory sig = _sign(PK, verifier.digest(ref, dest, hasTag, tag, vu));
        (AttestationParityVerifier.Leg[] memory legs, AttestationParityVerifier.Attestation[] memory atts) =
            _one(dest, hasTag, tag, vu, sig);
        verifier.verify(ref, legs, atts);
    }

    function testFuzz_RejectsHighSTwin(bytes32 ref, address dest, uint64 tag, uint32 dt) public {
        uint64 vu = _after(dt, 1, 86_400);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(PK, verifier.digest(ref, dest, true, tag, vu));
        // The twin recovers the same key through the precompile; only the
        // explicit low-s rule stops it.
        uint8 v2 = v == 27 ? 28 : 27;
        bytes32 s2 = bytes32(ORDER - uint256(s));
        assertEq(ecrecover(verifier.digest(ref, dest, true, tag, vu), v2, r, s2), verifier.attester());
        (AttestationParityVerifier.Leg[] memory legs, AttestationParityVerifier.Attestation[] memory atts) =
            _one(dest, true, tag, vu, abi.encodePacked(r, s2, v2));
        vm.expectRevert(_err(ATTESTATION_INVALID));
        verifier.verify(ref, legs, atts);
    }

    function testFuzz_RejectsAnyChangedBinding(bytes32 ref, address dest, uint64 tag, uint32 dt, uint8 which) public {
        uint64 vu = _after(dt, 1, 86_399);
        bytes memory sig = _sign(PK, verifier.digest(ref, dest, true, tag, vu));
        which = uint8(bound(which, 0, 4));
        bytes32 ref2 = ref;
        address dest2 = dest;
        bool hasTag2 = true;
        uint64 tag2 = tag;
        uint64 vu2 = vu;
        if (which == 0) ref2 = ref ^ bytes32(uint256(1));
        if (which == 1) dest2 = address(uint160(dest) ^ 1);
        if (which == 2) tag2 = tag ^ 1;
        if (which == 3) (hasTag2, tag2) = (false, 0);
        if (which == 4) vu2 = vu + 1;
        if (which == 3 && tag == 0) {
            // has_tag still differs, which is the point of the flag.
            assertTrue(verifier.digest(ref, dest, true, 0, vu) != verifier.digest(ref, dest, false, 0, vu));
        }
        (AttestationParityVerifier.Leg[] memory legs, AttestationParityVerifier.Attestation[] memory atts) =
            _one(dest2, hasTag2, tag2, vu2, sig);
        vm.expectRevert(_err(ATTESTATION_INVALID));
        verifier.verify(ref2, legs, atts);
    }

    function testFuzz_RejectsArbitraryBytes(bytes32 r, bytes32 s, uint8 v, bytes32 ref, address dest) public {
        bytes memory sig = abi.encodePacked(r, s, v);
        (AttestationParityVerifier.Leg[] memory legs, AttestationParityVerifier.Attestation[] memory atts) =
            _one(dest, false, 0, NOW + 60, sig);
        vm.expectRevert(_err(ATTESTATION_INVALID));
        verifier.verify(ref, legs, atts);
    }

    function testFuzz_RejectsAnyOtherLength(bytes memory sig, bytes32 ref, address dest) public {
        vm.assume(sig.length != 65);
        (AttestationParityVerifier.Leg[] memory legs, AttestationParityVerifier.Attestation[] memory atts) =
            _one(dest, false, 0, NOW + 60, sig);
        vm.expectRevert(_err(ATTESTATION_INVALID));
        verifier.verify(ref, legs, atts);
    }

    function testFuzz_ExpiryAndWindow(uint64 vu) public {
        address dest = makeAddr("dest");
        bytes memory sig = _sign(PK, verifier.digest(bytes32(0), dest, false, 0, vu));
        (AttestationParityVerifier.Leg[] memory legs, AttestationParityVerifier.Attestation[] memory atts) =
            _one(dest, false, 0, vu, sig);
        if (vu <= NOW) {
            vm.expectRevert(_err(ATTESTATION_EXPIRED));
        } else if (vu - NOW > 86_400) {
            vm.expectRevert(_err(ATTESTATION_INVALID));
        }
        verifier.verify(bytes32(0), legs, atts);
    }
}

/// Drives the verifier with forged attestations of every kind and records
/// any that were accepted.
contract ForgeryHandler is AttestationFixtureBase {
    AttestationParityVerifier public immutable VERIFIER;
    uint256 internal immutable ATTESTER_PK;
    uint256 internal immutable OTHER_PK;
    uint256 public accepted;
    uint256 public attempts;

    constructor(AttestationParityVerifier verifier, uint256 attesterPk, uint256 otherPk) {
        VERIFIER = verifier;
        ATTESTER_PK = attesterPk;
        OTHER_PK = otherPk;
    }

    function forge(bytes32 ref, address dest, uint64 tag, uint8 kind, bytes32 noise) external {
        uint64 vu = NOW + 3600;
        bytes32 d = VERIFIER.digest(ref, dest, true, tag, vu);
        kind = uint8(bound(kind, 0, 4));
        bytes memory sig;
        if (kind == 0) sig = _sign(OTHER_PK, d);
        // The real key over some other digest.
        if (kind == 1) sig = _sign(ATTESTER_PK, keccak256(abi.encode(d, noise)));
        if (kind == 2) {
            (uint8 v, bytes32 r, bytes32 s) = vm.sign(ATTESTER_PK, d);
            sig = abi.encodePacked(r, bytes32(ORDER - uint256(s)), v == 27 ? uint8(28) : uint8(27));
        }
        if (kind == 3) sig = abi.encodePacked(noise, keccak256(abi.encode(noise)), uint8(27));
        if (kind == 4) {
            (uint8 v, bytes32 r, bytes32 s) = vm.sign(ATTESTER_PK, d);
            sig = abi.encodePacked(r, s, v + 2);
        }
        (AttestationParityVerifier.Leg[] memory legs, AttestationParityVerifier.Attestation[] memory atts) =
            _one(dest, true, tag, vu, sig);
        ++attempts;
        try VERIFIER.verify(ref, legs, atts) {
            ++accepted;
        } catch (bytes memory reason) {
            require(keccak256(reason) == keccak256(_err(ATTESTATION_INVALID)), "unexpected revert");
        }
    }
}

contract AttestationParityInvariantTest is AttestationFixtureBase {
    ForgeryHandler internal handler;

    function setUp() public {
        vm.warp(NOW);
        uint256 pk = uint256(keccak256("UJIRA/TEST-ONLY/S18/fuzz-attester"));
        AttestationParityVerifier verifier = new AttestationParityVerifier(_uncompressed(pk), keccak256("policy"));
        handler = new ForgeryHandler(verifier, pk, uint256(keccak256("UJIRA/TEST-ONLY/S18/fuzz-other")));
        targetContract(address(handler));
    }

    function invariant_NoForgeryIsAccepted() public view {
        assertEq(handler.accepted(), 0);
    }
}
