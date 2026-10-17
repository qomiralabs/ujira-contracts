// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs
pragma solidity 0.8.37;

/// Spike only, kept under test/ so it never ships. Verifies per-leg recipient
/// attestations the way the mandate will, so the shared fixture can be run
/// here and on Soroban and every decision compared.
contract AttestationParityVerifier {
    struct Leg {
        address destination;
        bool hasTag;
        uint64 tag;
        uint256 amount;
    }

    /// signature is r || s || v, 65 bytes.
    struct Attestation {
        uint64 validUntil;
        bytes signature;
    }

    error MandateError(uint32 code);

    event AttestationPolicySet(address attester, bytes32 countryPolicyHash);

    uint32 internal constant CONFIG_INVALID = 34;
    uint32 internal constant ATTESTATION_INVALID = 51;
    uint32 internal constant ATTESTATION_EXPIRED = 52;

    uint256 internal constant MAX_ATTESTATION_VALIDITY = 86_400;
    uint256 internal constant ORDER = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141;
    uint256 internal constant HALF_ORDER = 0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0;
    bytes15 internal constant DOMAIN = "UJIRA/ATTEST/V1";

    // Storage, not immutables: the mandate lets the principal rotate both, so
    // measuring with storage reads gives the cost the mandate will pay.
    // slither-disable-next-line immutable-states
    address public attester;
    // slither-disable-next-line immutable-states
    bytes32 public policyHash;

    /// The key is stored as an address because ecrecover returns one. The
    /// Soroban side keeps the 65 byte key; both come from the same key bytes.
    constructor(bytes memory attesterKey, bytes32 countryPolicyHash) {
        if (attesterKey.length != 65 || attesterKey[0] != 0x04) revert MandateError(CONFIG_INVALID);
        bytes memory xy = new bytes(64);
        for (uint256 i = 0; i < 64; ++i) {
            xy[i] = attesterKey[i + 1];
        }
        attester = address(uint160(uint256(keccak256(xy))));
        policyHash = countryPolicyHash;
        emit AttestationPolicySet(attester, countryPolicyHash);
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
                policyHash,
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

    /// The request-time attestation check: one attestation per leg, in leg
    /// order, first failure wins.
    function verify(bytes32 recipientRef, Leg[] calldata legs, Attestation[] calldata attestations) external view {
        if (legs.length != attestations.length) revert MandateError(ATTESTATION_INVALID);
        for (uint256 i = 0; i < legs.length; ++i) {
            _verifyLeg(recipientRef, legs[i], attestations[i]);
        }
    }

    /// The precompile accepts high s, so malleability is closed here, and r
    /// and s are range checked so a malformed signature fails the same way on
    /// both chains before recovery is even tried.
    function scalarsInRange(uint256 r, uint256 s) public pure returns (bool) {
        return r != 0 && r < ORDER && s != 0 && s <= HALF_ORDER;
    }

    function _verifyLeg(bytes32 recipientRef, Leg calldata leg, Attestation calldata att) internal view {
        // Expiry is a rule on block time by design; a sequencer can shift it
        // by seconds, against a validity window of up to a day.
        // slither-disable-next-line timestamp
        if (block.timestamp >= att.validUntil) revert MandateError(ATTESTATION_EXPIRED);
        // validUntil > now here, so the subtraction cannot underflow; no
        // now + MAX sum either, matching the Soroban arithmetic exactly.
        // slither-disable-next-line timestamp
        if (att.validUntil - block.timestamp > MAX_ATTESTATION_VALIDITY) revert MandateError(ATTESTATION_INVALID);

        bytes calldata sig = att.signature;
        if (sig.length != 65) revert MandateError(ATTESTATION_INVALID);
        uint8 v = uint8(sig[64]);
        if (v != 27 && v != 28) revert MandateError(ATTESTATION_INVALID);
        bytes32 r = bytes32(sig[0:32]);
        bytes32 s = bytes32(sig[32:64]);
        if (!scalarsInRange(uint256(r), uint256(s))) revert MandateError(ATTESTATION_INVALID);

        address recovered =
            ecrecover(digest(recipientRef, leg.destination, leg.hasTag, leg.tag, att.validUntil), v, r, s);
        if (recovered == address(0) || recovered != attester) revert MandateError(ATTESTATION_INVALID);
    }
}
