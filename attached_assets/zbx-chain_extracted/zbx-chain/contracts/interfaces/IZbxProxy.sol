// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxProxy — Interface for ZbxProxy transparent upgradeable proxy.
interface IZbxProxy {
    event Upgraded(address indexed implementation);
    event AdminChanged(address previousAdmin, address newAdmin);

    function implementation() external view returns (address);
    function admin() external view returns (address);
    function upgradeTo(address newImplementation) external;
    function upgradeToAndCall(address newImplementation, bytes calldata data) external payable;
    function changeAdmin(address newAdmin) external;
}
