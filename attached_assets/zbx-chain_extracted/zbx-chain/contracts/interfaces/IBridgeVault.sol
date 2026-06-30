// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

/// @title IBridgeVault — public surface of the Zebvix bridge vault on BSC
/// @notice Users call `lock()` to send wrapped ZBX from BSC back to Zebvix L1.
///         The off-chain relayer watches `Locked` events, then submits a
///         matching `BridgeIn` admin tx on Zebvix to credit the recipient.
interface IBridgeVault {
    // ---------------------------------------------------------------------
    // User → vault (BSC → Zebvix path)
    // ---------------------------------------------------------------------

    /// @notice Burn `amount` of wrapped ZBX from the caller and emit a
    ///         `Locked` event for the off-chain relayer to pick up.
    /// @param  amount       Amount in 18-decimal wei.
    /// @param  zebvixDest   20-byte recipient address on Zebvix L1.
    /// @return seq          Monotonic vault-side sequence number; combined
    ///                      with `block.chainid` it forms the unique
    ///                      `source_tx_hash` Zebvix uses for replay
    ///                      protection.
    function lock(uint256 amount, bytes calldata zebvixDest)
        external
        returns (uint64 seq);

    /// @notice Same as `lock()` but for users who already held native BNB
    ///         and want to swap-and-bridge in one tx (router-friendly).
    function lockWithPermit(
        uint256 amount,
        bytes calldata zebvixDest,
        uint256 deadline,
        uint8 v,
        bytes32 r,
        bytes32 s
    ) external returns (uint64 seq);

    // ---------------------------------------------------------------------
    // Multisig oracle → vault (Zebvix → BSC path)
    // ---------------------------------------------------------------------

    /// @notice Called by `BridgeMultisig` after relayers prove a Zebvix
    ///         `BridgeOutEvent`. Mints wrapped ZBX to `to`. Idempotent —
    ///         each `zebvixSeq` may only be processed once.
    /// @param  to         Recipient on BSC.
    /// @param  amount     Amount in 18-decimal wei.
    /// @param  zebvixSeq  Sequence number from Zebvix `BridgeOutEvent`.
    function executeMint(address to, uint256 amount, uint64 zebvixSeq) external;

    // ---------------------------------------------------------------------
    // Read helpers for indexers and frontends
    // ---------------------------------------------------------------------

    function token() external view returns (address);
    function multisig() external view returns (address);
    function paused() external view returns (bool);
    function totalLocked() external view returns (uint256);
    function nextSeq() external view returns (uint64);
    function isZebvixSeqProcessed(uint64 zebvixSeq) external view returns (bool);

    // ---------------------------------------------------------------------
    // Events
    // ---------------------------------------------------------------------

    /// @notice Off-chain relayer watches this. Emit conditions:
    ///         - `lock()` or `lockWithPermit()` called by an EOA
    ///         - tokens successfully burned via `IZBX.bridgeBurnFrom`
    event Locked(
        address indexed from,
        uint256 amount,
        bytes zebvixDest,
        uint64 indexed seq
    );

    /// @notice Emitted after `executeMint()` mints wrapped ZBX to user.
    event Minted(address indexed to, uint256 amount, uint64 indexed zebvixSeq);

    /// @notice Emergency pause toggled by founder / multisig.
    event PausedSet(bool isPaused);
}