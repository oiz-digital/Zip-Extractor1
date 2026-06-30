// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxVaultRegistry — Interface for ZbxVaultRegistry yield vault discovery and metadata registry.
interface IZbxVaultRegistry {
    struct VaultInfo {
        address vault;
        address asset;
        string  name;
        string  strategyType;
        bool    active;
        uint256 tvl;
        uint256 apy;
        uint256 registeredAt;
    }

    event VaultRegistered(address indexed vault, address indexed asset, string name);
    event VaultDeactivated(address indexed vault);
    event VaultUpdated(address indexed vault, uint256 tvl, uint256 apy);

    function registerVault(address vault, address asset, string calldata name, string calldata strategyType) external;
    function deactivateVault(address vault) external;
    function updateMetrics(address vault, uint256 tvl, uint256 apy) external;
    function getVault(address vault) external view returns (VaultInfo memory);
    function allVaults() external view returns (address[] memory);
    function activeVaults() external view returns (address[] memory);
    function vaultsByAsset(address asset) external view returns (address[] memory);
}
