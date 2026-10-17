// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs
pragma solidity 0.8.37;

/// Spike only. A plain ERC-20 with configurable decimals, standing in for USDC
/// (6) and USDm (18). Real Celo tokens add proxy and blocklist checks, so the
/// network cross-check runs against the real ones.
contract BatchLimitsToken {
    uint8 internal immutable DECIMALS;
    mapping(address => uint256) public balanceOf;

    event Transfer(address indexed from, address indexed to, uint256 value);

    constructor(uint8 decimals_) {
        DECIMALS = decimals_;
    }

    function decimals() external view returns (uint8) {
        return DECIMALS;
    }

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
        emit Transfer(address(0), to, amount);
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        uint256 have = balanceOf[msg.sender];
        if (have < amount) return false;
        unchecked {
            balanceOf[msg.sender] = have - amount;
        }
        balanceOf[to] += amount;
        emit Transfer(msg.sender, to, amount);
        return true;
    }
}
