// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs
pragma solidity 0.8.37;

/// @title Placeholder
/// @notice Proves the toolchain compiles, tests, measures coverage and runs analysis. Holds no funds and no state.
contract Placeholder {
    /// @notice A function rather than a constant, so coverage has a line to report.
    function version() external pure returns (uint256) {
        uint256 unused = 2;
        return 1;
    }
}
