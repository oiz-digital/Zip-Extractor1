// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

/// @title IZBX — minimal interface for the wrapped ZBX token on external chains
/// @notice Standard ERC-20 surface plus mint/burn hooks reserved for the
///         bridge vault contract. Compatible with BEP-20 (BNB Chain),
///         ERC-20 (Ethereum, Polygon, Arbitrum), and any other EVM L2.
interface IZBX {
    // ---------------------------------------------------------------------
    // ERC-20 standard
    // ---------------------------------------------------------------------

    event Transfer(address indexed from, address indexed to, uint256 value);
    event Approval(address indexed owner, address indexed spender, uint256 value);

    function name() external view returns (string memory);
    function symbol() external view returns (string memory);
    function decimals() external view returns (uint8);

    function totalSupply() external view returns (uint256);
    function balanceOf(address account) external view returns (uint256);
    function allowance(address owner, address spender) external view returns (uint256);

    function transfer(address to, uint256 amount) external returns (bool);
    function approve(address spender, uint256 amount) external returns (bool);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);

    // ---------------------------------------------------------------------
    // Bridge extension — restricted to the BridgeVault contract.
    //
    // Authority model (post-architect-review fix):
    //   - `vault` (set once via `setVault(address)`, then locked) is the
    //     ONLY caller allowed to invoke `bridgeMint` / `bridgeBurnFrom`.
    //   - The multisig oracle never touches the token directly — it
    //     authorises mints by calling `BridgeVault.executeMint(...)`,
    //     which then calls `bridgeMint` with the canonical Zebvix seq.
    // ---------------------------------------------------------------------

    /// @notice Mint `amount` wrapped ZBX to `to`, tagged with `zebvixSeq`
    ///         from the originating Zebvix `BridgeOutEvent`. Vault-only.
    /// @param  to         Recipient on this chain.
    /// @param  amount     Amount in 18-decimal wei (matches Zebvix native ZBX).
    /// @param  zebvixSeq  Sequence number from Zebvix — emitted in event for
    ///                    indexers/relayers to correlate bridge legs.
    function bridgeMint(address to, uint256 amount, uint64 zebvixSeq) external;

    /// @notice Burn `amount` from `account`, spending the vault's allowance.
    ///         Vault-only. Used during `BridgeVault.lock()` so circulating
    ///         BSC supply contracts when funds head back to Zebvix L1,
    ///         preserving the 1:1 lock invariant.
    /// @param  account    Token holder whose balance shrinks.
    /// @param  amount     Amount in 18-decimal wei.
    /// @param  zebvixDest 20-byte recipient address on Zebvix L1 (event tag).
    function bridgeBurnFrom(
        address account,
        uint256 amount,
        bytes calldata zebvixDest
    ) external;

    // ---------------------------------------------------------------------
    // Events for off-chain indexers
    // ---------------------------------------------------------------------

    event BridgeMint(address indexed to, uint256 amount, uint64 indexed zebvixSeq);
    event BridgeBurn(address indexed from, uint256 amount, bytes zebvixDest);

    // ---------------------------------------------------------------------
    // Vault wiring (one-time-set then locked)
    // ---------------------------------------------------------------------

    function vault() external view returns (address);
    function vaultLocked() external view returns (bool);
    event VaultSet(address indexed vault);
}