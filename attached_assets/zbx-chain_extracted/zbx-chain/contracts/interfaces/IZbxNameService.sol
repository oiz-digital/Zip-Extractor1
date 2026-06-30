// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxNameService — Interface for ZbxNameService on-chain name registry (.zbx domains).
interface IZbxNameService {
    event Registered(string name, address indexed owner, uint256 expiry);
    event Renewed(string name, address indexed owner, uint256 newExpiry);
    event Transferred(string name, address indexed from, address indexed to);
    event RecordSet(string name, string key, string value);
    event SubdomainSet(string name, string subdomain, address target);

    error NameTaken();
    error NameExpired();
    error NotNameOwner();
    error InvalidName();
    error InsufficientFee();
    error ZeroAddress();

    function register(string calldata name, uint256 durationYears) external payable;
    function renew(string calldata name, uint256 durationYears) external payable;
    function transfer(string calldata name, address to) external;
    function setRecord(string calldata name, string calldata key, string calldata value) external;
    function setSubdomain(string calldata name, string calldata subdomain, address target) external;
    function resolve(string calldata name) external view returns (address);
    function getRecord(string calldata name, string calldata key) external view returns (string memory);
    function getExpiry(string calldata name) external view returns (uint256);
    function ownerOf(string calldata name) external view returns (address);
    function rentPrice(string calldata name, uint256 durationYears) external view returns (uint256);
}
