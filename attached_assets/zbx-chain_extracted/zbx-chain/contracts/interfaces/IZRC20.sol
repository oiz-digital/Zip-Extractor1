// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZRC20 — ZRC-20 Token Standard Interface
/// @notice ZRC-20 is the fungible token standard for Zebvix Chain (Chain ID 8989 mainnet / 8990 testnet+devnet).
///         It is fully EVM-compatible (superset of ERC-20) with three extensions
///         built into the base standard:
///           1. EIP-2612 Permit (gasless approvals via off-chain signatures)
///           2. Batch transfer (send to multiple recipients in one tx)
///           3. On-chain metadata (name, symbol, decimals, logo URI)
///
/// @dev All ZRC-20 tokens MUST implement this interface.
///      Deployment on ZBX Chain costs approximately 0.003–0.008 ZBX.

interface IZRC20 {

    // ─── Events ────────────────────────────────────────────────────────────

    /// @notice Emitted when `value` tokens are moved from `from` to `to`.
    event Transfer(address indexed from, address indexed to, uint256 value);

    /// @notice Emitted when `spender` is allowed to spend `value` tokens on behalf of `owner`.
    event Approval(address indexed owner, address indexed spender, uint256 value);

    /// @notice Emitted when permit is used (gasless approval).
    event Permit(address indexed owner, address indexed spender, uint256 value, uint256 deadline);

    /// @notice Emitted on batch transfer.
    event BatchTransfer(address indexed from, address[] to, uint256[] values);

    /// @notice Emitted when the on-chain logo URI is updated by the token owner.
    event LogoURIUpdated(string oldURI, string newURI);

    // ─── ERC-20 Core ───────────────────────────────────────────────────────

    function name()        external view returns (string memory);
    function symbol()      external view returns (string memory);
    function decimals()    external view returns (uint8);
    function totalSupply() external view returns (uint256);
    function balanceOf(address account)                          external view returns (uint256);
    function allowance(address owner, address spender)           external view returns (uint256);
    function transfer(address to, uint256 value)                 external returns (bool);
    function approve(address spender, uint256 value)             external returns (bool);
    function transferFrom(address from, address to, uint256 value) external returns (bool);

    // ─── ZRC-20 Extension: Batch Transfer ─────────────────────────────────

    /// @notice Transfer tokens to multiple recipients in a single transaction.
    ///         Saves ~21 000 gas per additional recipient vs. separate transfers.
    /// @param  to     Array of recipient addresses.
    /// @param  values Array of amounts (must match `to` length).
    function batchTransfer(address[] calldata to, uint256[] calldata values) external returns (bool);

    // ─── ZRC-20 Extension: EIP-2612 Permit ────────────────────────────────

    /// @notice Approve by signature — no separate approve tx needed.
    function permit(
        address owner,
        address spender,
        uint256 value,
        uint256 deadline,
        uint8   v,
        bytes32 r,
        bytes32 s
    ) external;

    /// @notice EIP-712 domain separator for this token.
    function DOMAIN_SEPARATOR() external view returns (bytes32);

    /// @notice EIP-2612 nonce for `owner` (incremented after each permit).
    function nonces(address owner) external view returns (uint256);

    // ─── ZRC-20 Extension: Metadata ────────────────────────────────────────

    /// @notice Optional IPFS / HTTPS URI pointing to a 256×256 PNG token logo.
    function logoURI() external view returns (string memory);

    /// @notice Returns all token metadata in one call (saves RPC round trips).
    function tokenInfo() external view returns (
        string memory tokenName,
        string memory tokenSymbol,
        uint8         tokenDecimals,
        uint256       supply,
        address       tokenOwner,
        string        memory logo
    );
}