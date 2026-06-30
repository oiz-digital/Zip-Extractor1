// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxOptions — Interface for ZbxOptions on-chain options protocol.
interface IZbxOptions {
    enum OptionType { Call, Put }

    struct Option {
        address underlying;
        OptionType optionType;
        uint256 strikePrice;
        uint256 expiry;
        uint256 premium;
        address writer;
        bool    exercised;
        bool    expired;
    }

    event OptionWritten(bytes32 indexed optionId, address indexed writer, address underlying, OptionType optionType, uint256 strike, uint256 expiry);
    event OptionBought(bytes32 indexed optionId, address indexed buyer, uint256 premium);
    event OptionExercised(bytes32 indexed optionId, address indexed holder, uint256 payout);
    event OptionExpired(bytes32 indexed optionId);

    function writeOption(address underlying, OptionType optionType, uint256 strikePrice, uint256 expiry, uint256 size) external payable returns (bytes32 optionId);
    function buyOption(bytes32 optionId) external payable;
    function exercise(bytes32 optionId) external;
    function expireOption(bytes32 optionId) external;
    function getOption(bytes32 optionId) external view returns (Option memory);
    function holderOf(bytes32 optionId) external view returns (address);
}
