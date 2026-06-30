// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxLiquidStaking — Interface for ZbxLiquidStaking liquid staking derivative (zbxZBX / stZBX).
interface IZbxLiquidStaking {
    event Staked(address indexed user, uint256 zbxIn, uint256 stZbxOut);
    event Unstaked(address indexed user, uint256 stZbxIn, uint256 zbxOut);
    event RewardsDistributed(uint256 amount);
    event ValidatorAdded(address indexed validator);
    event ValidatorRemoved(address indexed validator);

    function stake() external payable returns (uint256 stZbxMinted);
    function requestUnstake(uint256 stZbxAmount) external returns (uint256 requestId);
    function claimUnstake(uint256 requestId) external returns (uint256 zbxOut);
    function distributeRewards() external;
    function addValidator(address validator) external;
    function removeValidator(address validator) external;
    function stZbx() external view returns (address);
    function totalStaked() external view returns (uint256);
    function exchangeRate() external view returns (uint256);
    function unstakeDelay() external view returns (uint256);
    function pendingUnstake(uint256 requestId) external view returns (address owner, uint256 amount, uint256 claimableAt);
}
