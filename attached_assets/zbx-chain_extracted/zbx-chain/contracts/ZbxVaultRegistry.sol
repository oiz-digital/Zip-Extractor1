// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title ZbxVaultRegistry — canonical CDP storage layout for the
///        precompile-aware ZUSD vault, deployed at the well-known
///        address `0x000000000000000000000000000000000000_5455`.
///
/// @notice Task #7 — this contract is the authoritative storage layout
///         that precompile `0x0F` (`zusd_vault`) reads directly. The
///         layout is intentionally minimal so its slots are
///         deterministic: NO inheritance (no `Ownable`, no
///         `ReentrancyGuard`), NO state vars before `cdps`, so:
///
///             slot 0  →  mapping(address => CDP) cdps
///
///         The per-CDP base slot is therefore
///             keccak256(uint256(owner) ‖ uint256(0))
///         and the four CDP fields land at base + 0..3 in declaration
///         order: `collateral`, `debt`, `lastFeeIndex`, `openedAt`.
///
///         The precompile reads the first two (`collateral`, `debt`),
///         derives `c_ratio_bps` and `liquidation_price_e18` from the
///         oracle ZBX/USD feed, and packs four uint256 fields into a
///         128-byte BE response.
///
/// @dev    Writers (mint / burn / liquidate paths) live in the legacy
///         `ZusdVault.sol` and are migrated separately; that migration
///         is the open follow-up that retires `ZusdVault` and routes
///         all CDP state through this registry. Until then the
///         precompile reports "non-existent vault" (128 zero bytes)
///         for every owner — which is the documented graceful-degrade
///         path for the 0x0F precompile.
contract ZbxVaultRegistry {
    /// @dev    MUST stay slot 0. Adding state vars above this mapping
    ///         is a consensus break — the precompile pins
    ///         `CDPS_MAP_SLOT = 0` in `zbx-crypto::vault_state`.
    struct CDP {
        uint256 collateral;     // base + 0
        uint256 debt;           // base + 1
        uint256 lastFeeIndex;   // base + 2 (not read by 0x0F)
        uint256 openedAt;       // base + 3 (not read by 0x0F)
    }
    mapping(address => CDP) public cdps;

    /// @notice Precompile 0x0F address pin. Used by the genesis loader
    ///         to assert this contract was deployed at the canonical
    ///         address.
    address public constant CANONICAL_ADDRESS =
        address(uint160(0x5455));

    /// @notice Pure self-check used by the deployment script to make
    ///         the slot-0 invariant testable from Solidity. If you
    ///         add state above `cdps`, the precompile will read the
    ///         wrong slots; this view returns the slot a tooling test
    ///         can compare against `0`.
    function CDPS_MAP_SLOT() external pure returns (uint256 slot) {
        assembly { slot := cdps.slot }
    }
}
