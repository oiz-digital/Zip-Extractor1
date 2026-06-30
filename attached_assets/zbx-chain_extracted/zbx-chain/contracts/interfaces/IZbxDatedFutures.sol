// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxDatedFutures — Interface for ZbxDatedFutures fixed-expiry futures contracts.
interface IZbxDatedFutures {
    struct Contract {
        bytes32 marketId;
        uint256 strikePrice;
        uint256 expiry;
        bool    settled;
        uint256 settlementPrice;
    }

    event ContractCreated(bytes32 indexed contractId, bytes32 marketId, uint256 strikePrice, uint256 expiry);
    event ContractSettled(bytes32 indexed contractId, uint256 settlementPrice);
    event PositionOpened(bytes32 indexed contractId, address indexed trader, bool isLong, uint256 size);
    event PositionClosed(bytes32 indexed contractId, address indexed trader, int256 pnl);

    function createContract(bytes32 marketId, uint256 strikePrice, uint256 expiry) external returns (bytes32 contractId);
    function openPosition(bytes32 contractId, bool isLong, uint256 size) external payable;
    function closePosition(bytes32 contractId) external returns (int256 pnl);
    function settle(bytes32 contractId) external;
    function claimSettlement(bytes32 contractId) external;
    function getContract(bytes32 contractId) external view returns (Contract memory);
    function getPosition(bytes32 contractId, address trader) external view returns (bool isLong, uint256 size, uint256 margin);
}
