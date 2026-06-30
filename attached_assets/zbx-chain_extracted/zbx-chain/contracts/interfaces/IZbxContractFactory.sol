// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxContractFactory — Interface for ZbxContractFactory deterministic CREATE2 deployer.
interface IZbxContractFactory {
    event Deployed(address indexed deployer, address indexed deployed, bytes32 indexed salt);

    function deploy(bytes calldata bytecode, bytes32 salt) external returns (address deployed);
    function computeAddress(bytes calldata bytecode, bytes32 salt, address deployer) external pure returns (address);
    function deployAndInit(bytes calldata bytecode, bytes32 salt, bytes calldata initData) external returns (address deployed);
}
