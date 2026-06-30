// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title  IZBXGov — Read-only governance-token interface
/// @notice The minimal surface ZbxGovernor consumes from ZBXGov for
///         historical vote weighting. Both queries reference a strictly
///         past block (`blockNumber < block.number`) and MUST revert
///         otherwise — same convention as Compound/OZ Governor.
///
/// @dev    The full ZBXGov ABI (delegate, mint/burn, balance views, ERC-20)
///         is intentionally NOT mirrored here — Governor never calls any
///         non-snapshot function. Keeping this interface minimal also
///         avoids tight-coupling the Governor to ZBXGov's transferability
///         / soulbound semantics, so a future non-soulbound governance
///         token can drop in by implementing only these two functions.
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:since      S22a
interface IZBXGov {
    /// @notice Voting power assigned to `account` AFTER `blockNumber` was
    ///         mined (= delegated balance at end-of-block).
    /// @dev    Implementer MUST revert when `blockNumber >= block.number`
    ///         to enforce strictly-past snapshot semantics.
    function getPriorVotes(address account, uint256 blockNumber) external view returns (uint256);

    /// @notice Total ZBXGov supply (= sum of all delegated voting power
    ///         in the soulbound model) AFTER `blockNumber` was mined.
    /// @dev    Implementer MUST revert when `blockNumber >= block.number`.
    function totalSupplyAt(uint256 blockNumber) external view returns (uint256);
}
