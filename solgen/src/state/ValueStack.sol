//SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.0;

import "./Value.sol";
import "./ValueArray.sol";

struct ValueStack {
    ValueArray proved;
    bytes32 remainingHash;
}

library ValueStackLib {
    using ValueLib for Value;
    using ValueArrayLib for ValueArray;

    function hash(ValueStack memory stack) internal pure returns (bytes32 h) {
        h = stack.remainingHash;
        uint256 len = stack.proved.length();
        for (uint256 i = 0; i < len; i++) {
            h = keccak256(abi.encodePacked("Value stack:", stack.proved.get(i).hash(), h));
        }
    }

    function peek(ValueStack memory stack) internal pure returns (Value memory) {
        uint256 len = stack.proved.length();
        return stack.proved.get(len - 1);
    }

    function pop(ValueStack memory stack) internal pure returns (Value memory) {
        return stack.proved.pop();
    }

    function push(ValueStack memory stack, Value memory val) internal pure {
        return stack.proved.push(val);
    }

    function isEmpty(ValueStack memory stack) internal pure returns (bool) {
        return stack.proved.length() == 0 && stack.remainingHash == bytes32(0);
    }

    function hasProvenDepthLessThan(ValueStack memory stack, uint256 bound)
        internal
        pure
        returns (bool)
    {
        return stack.proved.length() < bound && stack.remainingHash == bytes32(0);
    }
}
