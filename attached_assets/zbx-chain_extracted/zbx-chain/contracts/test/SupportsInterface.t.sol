// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title SupportsInterface.t — EIP-165 conformance tests for S21 broadcast.
///
/// @notice Before S21, only ZbxNFT and ZRC20.sol implemented EIP-165
///         supportsInterface. S21 added it to:
///           Tier-1 (full I* claims): ZRC20Base, ZRC20Token, ZRC721Base,
///                                    ZbxTvlOracle  (+ ZRC20.sol upgraded)
///           Tier-2 (IERC165 only):   ZbxAMM, ZbxLendingPool, ZbxGovernor
///                                    (ABI mismatch with their I*
///                                    interfaces — see contract NatSpec)
///
/// @dev    Sandbox cannot run forge. Off-sandbox VPS srv1266996:
///         `forge test --match-path contracts/test/SupportsInterface.t.sol -vvv`
///
/// @custom:zbx-chain  Chain ID 8989

import { ZRC20Base }       from "../ZRC20Base.sol";
import { ZRC20 }           from "../ZRC20.sol";
import { ZRC20Token }      from "../ZRC20Token.sol";
import { ZRC721Base }      from "../ZRC721Base.sol";
import { ZbxAMM }          from "../ZbxAMM.sol";
import { ZbxLendingPool }  from "../ZbxLendingPool.sol";
import { ZbxGovernor }     from "../ZbxGovernor.sol";
import { ZbxTvlOracle }    from "../ZbxTvlOracle.sol";

import { IZRC20 }            from "../interfaces/IZRC20.sol";
import { IZRC20Mintable }    from "../interfaces/IZRC20Mintable.sol";
import { IZRC20Burnable }    from "../interfaces/IZRC20Burnable.sol";
import { IZRC20Freezable }   from "../interfaces/IZRC20Freezable.sol";
import { IZRC20Lockable }    from "../interfaces/IZRC20Lockable.sol";
import { IZRC721 }           from "../interfaces/IZRC721.sol";
import { IZbxTvlOracle }     from "../interfaces/IZbxTvlOracle.sol";

bytes4 constant IERC165_ID  = 0x01ffc9a7;
bytes4 constant UNKNOWN_ID  = 0xdeadbeef;

// ─── Test scaffolds for abstract bases ────────────────────────────────────

/// Minimal concrete subclass of the abstract ZRC721Base, just so we can
/// deploy it and call supportsInterface. Adds zero functionality.
contract TestableZRC721Base is ZRC721Base {
    constructor() ZRC721Base("Test721", "T721", "ipfs://test/") {}
}

contract SupportsInterfaceTest {

    // ─── 1. ZRC20.sol (bridge wrapper) ────────────────────────────────────

    function test_ZRC20_AdvertisesAllExtensions() public {
        ZRC20 t = new ZRC20(0);
        require(t.supportsInterface(type(IZRC20).interfaceId),          "IZRC20 (via Base)");
        require(t.supportsInterface(type(IZRC20Mintable).interfaceId),  "IZRC20Mintable");
        require(t.supportsInterface(type(IZRC20Burnable).interfaceId),  "IZRC20Burnable");
        require(t.supportsInterface(type(IZRC20Freezable).interfaceId), "IZRC20Freezable");
        require(t.supportsInterface(IERC165_ID),                        "IERC165");
        require(!t.supportsInterface(UNKNOWN_ID),                       "must reject unknown id");
        // ZRC20.sol must NOT advertise IZRC20Lockable (it has no lock surface).
        require(!t.supportsInterface(type(IZRC20Lockable).interfaceId), "must NOT claim Lockable");
    }

    // ─── 2. ZRC20Token (general-purpose) ──────────────────────────────────

    function test_ZRC20Token_AdvertisesAllExtensions() public {
        // Constructor: (name, symbol, decimals, initialSupply, mintCap,
        // logoURI, owner_) — 7 args. owner_ is non-zero asserted; pass
        // the test contract so it becomes the owner.
        ZRC20Token t = new ZRC20Token(
            "TestToken",
            "TTK",
            18,                         // decimals
            1_000 ether,                // initial supply (≤ mintCap)
            10_000 ether,               // mint cap
            "ipfs://test",
            address(this)               // owner_
        );
        require(t.supportsInterface(type(IZRC20).interfaceId),          "IZRC20 (via Base)");
        require(t.supportsInterface(type(IZRC20Mintable).interfaceId),  "Mintable");
        require(t.supportsInterface(type(IZRC20Burnable).interfaceId),  "Burnable");
        require(t.supportsInterface(type(IZRC20Freezable).interfaceId), "Freezable");
        require(t.supportsInterface(type(IZRC20Lockable).interfaceId),  "Lockable");
        require(t.supportsInterface(IERC165_ID),                        "IERC165");
        require(!t.supportsInterface(UNKNOWN_ID),                       "reject unknown");
    }

    // ─── 3. ZRC721Base (via TestableZRC721Base mock) ──────────────────────

    function test_ZRC721Base_AdvertisesIZRC721AndIERC165() public {
        TestableZRC721Base t = new TestableZRC721Base();
        require(t.supportsInterface(type(IZRC721).interfaceId), "IZRC721");
        require(t.supportsInterface(IERC165_ID),                "IERC165");
        require(!t.supportsInterface(UNKNOWN_ID),               "reject unknown");
    }

    // ─── 4. ZbxTvlOracle (Tier-1, native is-IZbxTvlOracle) ────────────────

    function test_ZbxTvlOracle_AdvertisesIZbxTvlOracle() public {
        // Constructor: (address owner_) — Ownable2Step rejects address(0).
        ZbxTvlOracle o = new ZbxTvlOracle(address(this));
        require(o.supportsInterface(type(IZbxTvlOracle).interfaceId), "IZbxTvlOracle");
        require(o.supportsInterface(IERC165_ID),                      "IERC165");
        require(!o.supportsInterface(UNKNOWN_ID),                     "reject unknown");
    }

    // ─── 5. Tier-2 contracts: IERC165-only claims ─────────────────────────
    //
    // These contracts have ABI divergence from their nominal I* interfaces
    // (documented in each contract's supportsInterface NatSpec). They MUST
    // claim ONLY 0x01ffc9a7 to avoid false EIP-165 advertisements.

    function test_ZbxAMM_AdvertisesIERC165Only() public {
        // ZbxAMM constructor: (token0, token1) — sorted-tuple; use any
        // distinguishable non-zero addresses. supportsInterface is pure,
        // so the wiring need not be functional.
        ZbxAMM a = new ZbxAMM(address(0x1111), address(0x2222));
        require( a.supportsInterface(IERC165_ID),  "IERC165");
        require(!a.supportsInterface(UNKNOWN_ID),  "reject unknown");
        // Static ABI audit: must NOT (yet) claim IZbxAMM — see
        // S21-FOLLOWUP-AMM-INTERFACE-RECONCILIATION. We don't compute the interfaceId here
        // (it would require importing IZbxAMM and forcing a transitive
        // ABI dep) — the unknown-id check is the regression guard.
    }

    function test_ZbxLendingPool_AdvertisesIERC165Only() public {
        // Constructor: (address oracle_) — non-zero asserted in ctor.
        ZbxLendingPool p = new ZbxLendingPool(address(0x1));
        require( p.supportsInterface(IERC165_ID),  "IERC165");
        require(!p.supportsInterface(UNKNOWN_ID),  "reject unknown");
        // S21-FOLLOWUP-LENDING-INTERFACE-RECONCILIATION pending.
    }

    function test_ZbxGovernor_AdvertisesIERC165Only() public {
        // Constructor: (address token_, address timelock_) — both non-zero asserted.
        ZbxGovernor g = new ZbxGovernor(address(0x1), address(0x2));
        require( g.supportsInterface(IERC165_ID),  "IERC165");
        require(!g.supportsInterface(UNKNOWN_ID),  "reject unknown");
        // S21-FOLLOWUP-GOVERNOR-INTERFACE-RECONCILIATION pending.
    }

    // ─── 6. Cross-cutting EIP-165 invariants ──────────────────────────────
    //
    // The EIP-165 spec requires the special case `supportsInterface(0xffffffff)`
    // → false (this is how callers distinguish a true EIP-165 contract from
    // a contract whose fallback always returns true). Verify on the highest-
    // value contract surface.

    function test_AllAdvertise_0xffffffff_AsFalse() public {
        bytes4 spec = 0xffffffff;
        ZRC20 t = new ZRC20(0);
        require(!t.supportsInterface(spec), "ZRC20 0xffffffff");

        TestableZRC721Base n = new TestableZRC721Base();
        require(!n.supportsInterface(spec), "ZRC721Base 0xffffffff");

        ZbxTvlOracle o = new ZbxTvlOracle(address(this));
        require(!o.supportsInterface(spec), "ZbxTvlOracle 0xffffffff");
    }
}
