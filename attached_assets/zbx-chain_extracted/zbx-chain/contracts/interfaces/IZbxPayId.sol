// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

/**
 * @title IZbxPayId
 * @notice Interface for ZBX Pay ID registry.
 *
 * Pay ID format: ali@zbx
 * Sub-ID format: shop.ali@zbx
 */
interface IZbxPayId {
    function register(string calldata payId, address wallet) external payable;
    function resolve(string calldata payId) external view returns (address);
    function resolveChain(string calldata payId, uint256 chainId) external view returns (string memory);
    function reverseLookup(address wallet) external view returns (string memory);
    function isAvailable(string calldata payId) external view returns (bool);
    function updateWallet(string calldata payId, address newWallet) external;
    function setChainAddress(string calldata payId, uint256 chainId, string calldata addr) external;
    function transfer(string calldata payId, address newOwner) external;
    function issueSubId(string calldata parentId, string calldata subName, address to) external;
    function release(string calldata payId) external;
}