// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

/// @title IBridgeMultisig — public surface of the Zebvix bridge oracle multisig
/// @notice Off-chain relayers and front-end indexers integrate against this
///         interface. The implementation lives in `BridgeMultisig.sol`.
interface IBridgeMultisig {
    // ---------------------------------------------------------------------
    // Events
    // ---------------------------------------------------------------------

    event Voted(uint64 indexed zebvixSeq, address indexed relayer, uint64 count);
    event Executed(uint64 indexed zebvixSeq, address indexed to, uint256 amount);
    event RelayersUpdated(address[] relayers);
    event VaultSet(address indexed vault);
    event VaultLocked();
    event PausedSet(bool isPaused);
    event FounderTransferred(address indexed from, address indexed to);

    // ---------------------------------------------------------------------
    // Vote submission
    // ---------------------------------------------------------------------

    /// @notice Submit a single relayer's EIP-191 personal-sign signature
    ///         endorsing a Zebvix `BridgeOutEvent`. Once `threshold()`
    ///         distinct relayer signatures arrive for the same
    ///         `(zebvixSeq, to, amount)` tuple the multisig calls
    ///         `BridgeVault.executeMint(...)` and the user receives
    ///         wrapped ZBX on this chain.
    function submitMint(
        uint64 zebvixSeq,
        address to,
        uint256 amount,
        uint8 v,
        bytes32 r,
        bytes32 s
    ) external;

    /// @notice Batch-submit several relayer sigs in one tx. Stops early
    ///         once the quorum is reached so the post-execute sigs do not
    ///         revert the whole batch.
    function submitMintBatch(
        uint64 zebvixSeq,
        address to,
        uint256 amount,
        uint8[] calldata vs,
        bytes32[] calldata rs,
        bytes32[] calldata ss
    ) external;

    // ---------------------------------------------------------------------
    // Read helpers
    // ---------------------------------------------------------------------

    function vault() external view returns (address);
    function vaultLocked() external view returns (bool);
    function threshold() external view returns (uint256);
    function founder() external view returns (address);
    function paused() external view returns (bool);

    function isRelayer(address who) external view returns (bool);
    function relayers() external view returns (address[] memory);
    function relayerCount() external view returns (uint256);

    function votedBy(uint64 zebvixSeq, address relayer) external view returns (bool);

    /// @return count    Number of distinct relayer sigs received so far.
    /// @return to       Recipient address (set by the first valid sig).
    /// @return amount   Amount in 18-decimal wei (set by the first valid sig).
    /// @return executed True once the multisig has called `executeMint`.
    function tallies(uint64 zebvixSeq)
        external
        view
        returns (uint64 count, address to, uint256 amount, bool executed);

    // ---------------------------------------------------------------------
    // Founder + lifecycle ops
    // ---------------------------------------------------------------------

    function setVault(address _vault) external;
    function lockVault() external;
    function setRelayers(address[] calldata newSet) external;
    function setPaused(bool _p) external;
    function transferFounder(address newFounder) external;

    // ---------------------------------------------------------------------
    // Off-chain signing helper — pure, gas-free
    // ---------------------------------------------------------------------

    /// @notice The exact bytes a relayer signs (after personal-sign prefix).
    ///         Implementations must match `keccak256(abi.encode(
    ///         DOMAIN_TAG, block.chainid, vault, zebvixSeq, to, amount))`.
    ///         Front-ends call this to keep their signing path in lockstep
    ///         with the on-chain digest computation.
    function digestFor(
        uint64 zebvixSeq,
        address to,
        uint256 amount
    ) external view returns (bytes32);
}