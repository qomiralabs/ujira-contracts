// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs
pragma solidity 0.8.30;

import {Placeholder} from "../src/Placeholder.sol";
import {Test} from "forge-std/Test.sol";

contract PlaceholderTest is Test {
    Placeholder internal placeholder;

    function setUp() public {
        placeholder = new Placeholder();
    }

    function test_Version() public view {
        assertEq(placeholder.version(), 1);
    }
}
