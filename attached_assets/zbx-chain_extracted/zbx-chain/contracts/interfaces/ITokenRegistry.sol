// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title ITokenRegistry — Interface for TokenRegistry whitelisted token directory.
interface ITokenRegistry {
    struct TokenInfo {
        address token;
        string  name;
        string  symbol;
        uint8   decimals;
        bool    active;
        uint256 registeredAt;
    }

    event TokenRegistered(address indexed token, string symbol);
    event TokenDeactivated(address indexed token);
    event TokenReactivated(address indexed token);

    function register(address token) external;
    function deactivate(address token) external;
    function reactivate(address token) external;
    function getToken(address token) external view returns (TokenInfo memory);
    function isRegistered(address token) external view returns (bool);
    function isActive(address token) external view returns (bool);
    function allTokens() external view returns (address[] memory);
    function activeTokens() external view returns (address[] memory);
}
