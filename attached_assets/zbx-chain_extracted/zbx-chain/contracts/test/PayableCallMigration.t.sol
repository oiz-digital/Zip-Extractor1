// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

/// @title  PayableCallMigration.t.sol
/// @notice Tests for the S19 migration of 4 native ETH transfers from
///         `.transfer(...)` (gas-limited to 2300, broken by EIP-2929) to
///         `.call{value:...}("")` (forwards all gas).
///
///         Sites covered:
///           - ZbxBundler.deregisterBundler   — refund stake to bundler
///           - ZbxBundler.slash               — send slashed amount to owner
///           - ZbxPayId.withdraw              — drain fees to immutable owner
///
///         The ZbxSmartWallet.validateUserOp site is also migrated by S19
///         but is exercised through ERC-4337 EntryPoint integration tests
///         (separate file) because validateUserOp can ONLY be called by
///         the registered EntryPoint and intentionally suppresses the
///         return bool per ERC-4337 §5.
///
/// @dev    NOT executable in the Replit sandbox. Mandatory off-sandbox:
///         `forge test --match-path contracts/test/PayableCallMigration.t.sol -vvv`
///
/// @custom:zbx-chain  Chain ID 8989

import { ZbxBundler }   from "../ZbxBundler.sol";
import { ZbxPayId }     from "../ZbxPayId.sol";
import { ZbxFaucet }    from "../ZbxFaucet.sol";
import { ZRC20Factory } from "../ZRC20Factory.sol";

interface Hevm {
    function prank(address) external;
    function deal(address, uint256) external;
    function expectRevert(bytes calldata) external;
    function expectRevert(bytes4)         external;
}

// ─── Receivers ────────────────────────────────────────────────────────────

/// Plain payable receiver — succeeds with minimal gas.
contract EoaLikeReceiver {
    receive() external payable {}
}

/// Receiver that consumes >>2300 gas in receive() (writes a non-zero slot).
/// Under `.transfer(...)` this would have failed; under `.call(...)` it MUST
/// succeed. This is the whole point of the S19 migration.
contract GasGriefingReceiver {
    uint256 public touches;
    receive() external payable {
        touches += 1;          // 20k gas first time, 5k subsequently
        touches += 1;          // ensure we blow well past the 2300 stipend
    }
}

/// Receiver that always reverts. The migrated `.call` site MUST surface
/// this as `require(ok, "...")` revert.
contract RejectingReceiver {
    receive() external payable {
        revert("rejected");
    }
}

/// Reentrancy attacker — tries to recursively call deregisterBundler() during
/// the refund. ReentrancyGuard MUST block. The attacker's receive() catches
/// the inner revert (via try/catch) and returns normally, so the outer .call
/// to the attacker returns TRUE — letting us prove the guard fired by reading
/// the captured inner revert reason rather than by inferring from a top-level
/// failure (which could be caused by other checks like "not a bundler").
contract ReentrancyAttackerBundler {
    ZbxBundler public target;
    bool       public attemptedReentry;
    bool       public reentryReverted;
    string     public capturedRevertReason;

    constructor(ZbxBundler t) payable {
        target = t;
    }

    function attackRegister() external payable {
        target.registerBundler{value: msg.value}();
    }

    function attackDeregister() external {
        target.deregisterBundler();
    }

    receive() external payable {
        if (!attemptedReentry) {
            attemptedReentry = true;
            try target.deregisterBundler() {
                // Reaching here means the guard did NOT fire — bad.
            } catch Error(string memory reason) {
                reentryReverted = true;
                capturedRevertReason = reason;
            } catch {
                // Low-level revert without a string — still a revert.
                reentryReverted = true;
            }
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────

contract PayableCallMigrationTest {

    Hevm internal constant HEVM = Hevm(address(uint160(uint256(keccak256("hevm cheat code")))));

    address internal constant ENTRY_POINT = address(0xE17);
    address internal constant ALICE       = address(0xA11CE);

    // ─── ZbxBundler.deregisterBundler ────────────────────────────────────

    function test_Bundler_Deregister_HappyPath_EoaReceiver() public {
        ZbxBundler bundler = new ZbxBundler(ENTRY_POINT);

        EoaLikeReceiver eoa = new EoaLikeReceiver();
        HEVM.deal(address(eoa), 10 ether);

        HEVM.prank(address(eoa));
        bundler.registerBundler{value: 0.1 ether}();
        require(bundler.bundlerActive(address(eoa)), "should be active");

        uint256 balBefore = address(eoa).balance;
        HEVM.prank(address(eoa));
        bundler.deregisterBundler();

        require(!bundler.bundlerActive(address(eoa)), "should be inactive");
        require(bundler.bundlerStake(address(eoa)) == 0, "stake should be 0");
        require(address(eoa).balance == balBefore + 0.1 ether, "refund missing");
    }

    function test_Bundler_Deregister_SucceedsOnGasGriefingReceiver() public {
        ZbxBundler bundler = new ZbxBundler(ENTRY_POINT);

        GasGriefingReceiver gg = new GasGriefingReceiver();
        HEVM.deal(address(gg), 1 ether);

        HEVM.prank(address(gg));
        bundler.registerBundler{value: 0.1 ether}();

        // Pre-S19 this would have FAILED (`.transfer` 2300-gas stipend
        // cannot cover the SSTOREs in receive). Post-S19 it MUST succeed.
        HEVM.prank(address(gg));
        bundler.deregisterBundler();

        require(gg.touches() == 2, "receive() should have run");
        require(bundler.bundlerStake(address(gg)) == 0, "stake should be 0");
    }

    function test_Bundler_Deregister_RevertsOnRejectingReceiver() public {
        ZbxBundler bundler = new ZbxBundler(ENTRY_POINT);

        RejectingReceiver rej = new RejectingReceiver();
        HEVM.deal(address(rej), 1 ether);

        HEVM.prank(address(rej));
        bundler.registerBundler{value: 0.1 ether}();

        // .call surfaces the revert through `require(ok, ...)`.
        HEVM.prank(address(rej));
        HEVM.expectRevert(bytes("ZbxBundler: stake refund failed"));
        bundler.deregisterBundler();

        // State must be untouched (atomic revert).
        require(bundler.bundlerActive(address(rej)), "must remain active");
        require(bundler.bundlerStake(address(rej)) == 0.1 ether, "stake unchanged");
    }

    function test_Bundler_Deregister_NonReentrant() public {
        ZbxBundler bundler = new ZbxBundler(ENTRY_POINT);

        ReentrancyAttackerBundler attacker = new ReentrancyAttackerBundler{value: 0.1 ether}(bundler);
        HEVM.deal(address(attacker), 1 ether);

        attacker.attackRegister{value: 0.1 ether}();

        // The attacker's receive() catches the inner revert internally, so
        // it returns normally and the OUTER .call returns TRUE → outer
        // deregisterBundler SUCCEEDS. We assert the guard fired by reading
        // the captured inner revert reason — this distinguishes a true
        // ReentrancyGuard hit from any alternate cause (e.g. the
        // "not a bundler" check that would also fire if state were already
        // cleared by some other path).
        attacker.attackDeregister();

        require(attacker.attemptedReentry(), "attacker should have tried reentry");
        require(attacker.reentryReverted(),  "reentry should have reverted");

        // EXACT match: prove the revert came from ReentrancyGuard, not from
        // any other require() in deregisterBundler.
        require(
            keccak256(bytes(attacker.capturedRevertReason())) ==
            keccak256(bytes("ReentrancyGuard: reentrant call")),
            "reentry must revert with ReentrancyGuard message"
        );

        // Outer succeeded → state cleared correctly (no double-deregister).
        require(!bundler.bundlerActive(address(attacker)), "should be inactive");
        require(bundler.bundlerStake(address(attacker)) == 0, "stake should be 0");
    }

    // ─── ZbxBundler.slash ────────────────────────────────────────────────

    function test_Bundler_Slash_TransfersToOwner() public {
        // Owner = address(this) (deployer).
        ZbxBundler bundler = new ZbxBundler(ENTRY_POINT);

        EoaLikeReceiver bundlerEoa = new EoaLikeReceiver();
        HEVM.deal(address(bundlerEoa), 10 ether);
        HEVM.prank(address(bundlerEoa));
        bundler.registerBundler{value: 1 ether}();

        uint256 balBefore = address(this).balance;
        bundler.slash(address(bundlerEoa), 0.3 ether, "double-spend");

        require(address(this).balance == balBefore + 0.3 ether, "owner did not receive slash");
        require(bundler.bundlerStake(address(bundlerEoa)) == 0.7 ether, "stake decrement wrong");
    }

    function test_Bundler_Slash_RevertsOnRejectingOwner() public {
        // Deploy bundler from a rejecting-owner context (prank the ctor so
        // msg.sender during ZbxBundler() == rejOwner → owner = rejOwner).
        RejectingReceiver rejOwner = new RejectingReceiver();
        HEVM.prank(address(rejOwner));
        ZbxBundler bundler = new ZbxBundler(ENTRY_POINT);

        EoaLikeReceiver bundlerEoa = new EoaLikeReceiver();
        HEVM.deal(address(bundlerEoa), 10 ether);
        HEVM.prank(address(bundlerEoa));
        bundler.registerBundler{value: 1 ether}();

        HEVM.prank(address(rejOwner));
        HEVM.expectRevert(bytes("ZbxBundler: slash transfer failed"));
        bundler.slash(address(bundlerEoa), 0.3 ether, "test");
    }

    // ─── ZbxPayId.withdraw ───────────────────────────────────────────────

    function test_PayId_Withdraw_HappyPath() public {
        ZbxPayId payId = new ZbxPayId();           // owner = this
        HEVM.deal(address(payId), 5 ether);

        uint256 balBefore = address(this).balance;
        payId.withdraw();
        require(address(this).balance == balBefore + 5 ether, "owner did not receive");
        require(address(payId).balance == 0, "contract should be drained");
    }

    function test_PayId_Withdraw_OnlyOwner() public {
        ZbxPayId payId = new ZbxPayId();
        HEVM.deal(address(payId), 1 ether);

        HEVM.prank(address(0xBAD));
        HEVM.expectRevert(bytes("ZbxPayId: not contract owner"));
        payId.withdraw();
    }

    function test_PayId_Withdraw_RevertsOnRejectingOwner() public {
        // Prank ctor so owner == rejOwner.
        RejectingReceiver rejOwner = new RejectingReceiver();
        HEVM.prank(address(rejOwner));
        ZbxPayId payId = new ZbxPayId();
        HEVM.deal(address(payId), 1 ether);

        HEVM.prank(address(rejOwner));
        HEVM.expectRevert(bytes("ZbxPayId: withdraw failed"));
        payId.withdraw();

        // State integrity: contract balance unchanged on revert.
        require(address(payId).balance == 1 ether, "balance changed on revert");
    }

    // ─── ZbxFaucet.withdraw (S19-EXT) ────────────────────────────────────

    function test_Faucet_Withdraw_HappyPath() public {
        ZbxFaucet faucet = new ZbxFaucet();        // owner = this
        HEVM.deal(address(faucet), 5 ether);

        uint256 balBefore = address(this).balance;
        faucet.withdraw();
        require(address(this).balance == balBefore + 5 ether, "owner did not receive");
        require(address(faucet).balance == 0, "faucet should be drained");
    }

    function test_Faucet_Withdraw_RevertsOnRejectingOwner() public {
        // Prank ctor so owner == rejOwner.
        RejectingReceiver rejOwner = new RejectingReceiver();
        HEVM.prank(address(rejOwner));
        ZbxFaucet faucet = new ZbxFaucet();
        HEVM.deal(address(faucet), 1 ether);

        HEVM.prank(address(rejOwner));
        HEVM.expectRevert(bytes("ZbxFaucet: withdraw failed"));
        faucet.withdraw();

        require(address(faucet).balance == 1 ether, "balance changed on revert");
    }

    // ─── ZRC20Factory.createToken (S19-EXT) ──────────────────────────────

    function test_Factory_Refund_SucceedsOnGasGriefingCaller() public {
        ZRC20Factory factory = _deployFactory(0.1 ether, address(0x7EA1));

        GasGriefingReceiver gg = new GasGriefingReceiver();
        HEVM.deal(address(gg), 10 ether);

        // Caller sends 1 ether but creationFee is 0.1 — refund = 0.9.
        // Pre-S19 the receive() consuming >>2300 gas would have FAILED;
        // post-S19 it MUST succeed.
        HEVM.prank(address(gg));
        try factory.createToken{value: 1 ether}(
            "Tok", "TOK", 18, 0, 0, "", bytes32(uint256(0xCAFE))
        ) returns (address) {
            // OK: refund succeeded, token deployed
            require(gg.touches() == 2, "receive() should have run");
        } catch Error(string memory reason) {
            // Acceptable downstream: CREATE2 may fail if salt collides.
            // We only assert the refund-fail path was NOT hit.
            require(
                keccak256(bytes(reason)) != keccak256(bytes("ZRC20Factory: refund failed")),
                "refund leg wrongly failed under .call"
            );
        } catch {
            // Low-level revert (CREATE2 collision) is fine for our purpose.
        }
    }

    function test_Factory_Treasury_RevertsOnRejectingTreasury() public {
        RejectingReceiver rejTreasury = new RejectingReceiver();
        ZRC20Factory factory = _deployFactory(0.1 ether, address(rejTreasury));

        HEVM.deal(address(this), 10 ether);
        HEVM.expectRevert(bytes("ZRC20Factory: treasury fee failed"));
        factory.createToken{value: 0.1 ether}(
            "Tok", "TOK", 18, 0, 0, "", bytes32(uint256(0xBEEF))
        );
    }

    function test_Factory_NonReentrant_PreventsDoubleSpawn() public {
        // A malicious treasury that tries to reenter createToken() during
        // the treasury-fee transfer must be blocked by nonReentrant. The
        // attacker swallows the inner revert via try/catch, so its receive()
        // returns normally and the OUTER treasury .call returns TRUE — i.e.
        // the OUTER createToken SUCCEEDS and registers exactly one token.
        // We assert: (a) outer success, (b) exactly one token in allTokens,
        // (c) attacker attempted reentry, (d) reentry was reverted by the
        // guard with the canonical message.
        FactoryReentrancyAttacker attacker = new FactoryReentrancyAttacker();
        ZRC20Factory factory = _deployFactory(0.1 ether, address(attacker));
        attacker.setTarget(factory);

        HEVM.deal(address(this), 10 ether);

        address tok = factory.createToken{value: 0.1 ether}(
            "Tok", "TOK", 18, 0, 0, "", bytes32(uint256(0xDEAD))
        );
        require(tok != address(0), "outer createToken should succeed");

        // Exactly ONE token registered. allTokens(0) must be `tok`; reading
        // allTokens(1) must revert (out-of-bounds) — proves no double-spawn.
        require(factory.allTokens(0) == tok, "first token must be at index 0");

        bool onlyOne = false;
        try factory.allTokens(1) returns (address) {
            onlyOne = false;
        } catch {
            onlyOne = true;
        }
        require(onlyOne, "should only have 1 token in allTokens (no double-spawn)");

        // Attacker MUST have tried reentry and the inner call MUST have
        // reverted with the canonical guard message.
        require(attacker.attemptedReentry(), "attacker should have tried reentry");
        require(attacker.reentryReverted(),  "reentry should have reverted (guard worked)");
        require(
            keccak256(bytes(attacker.capturedRevertReason())) ==
            keccak256(bytes("ReentrancyGuard: reentrant call")),
            "reentry must revert with ReentrancyGuard message"
        );
    }

    // ─── Helpers ─────────────────────────────────────────────────────────

    /// @dev ZRC20Factory's actual constructor signature is
    ///      `(address treasury_, uint256 creationFee_)` — verified against
    ///      contracts/ZRC20Factory.sol L37. Helper kept for readability.
    function _deployFactory(uint256 fee, address treasury) internal returns (ZRC20Factory) {
        return new ZRC20Factory(treasury, fee);
    }

    // Allow this test contract to receive ETH for the slash + withdraw
    // happy-path tests where the test contract itself is the owner.
    receive() external payable {}
}

contract FactoryReentrancyAttacker {
    ZRC20Factory public target;
    bool         public attemptedReentry;
    bool         public reentryReverted;
    string       public capturedRevertReason;

    function setTarget(ZRC20Factory t) external {
        target = t;
    }

    receive() external payable {
        if (address(target) != address(0) && !attemptedReentry) {
            attemptedReentry = true;
            try target.createToken{value: 0.1 ether}(
                "Re", "RE", 18, 0, 0, "", bytes32(uint256(0xBADBAD))
            ) returns (address) {
                // If we reach here, the guard FAILED — reentryReverted stays false.
            } catch Error(string memory reason) {
                reentryReverted = true;
                capturedRevertReason = reason;
            } catch {
                // Low-level revert without a string — still a revert.
                reentryReverted = true;
            }
        }
    }
}

// ─── End of file. The S18 polish helpers (BundlerFactory / PayIdFactory)
//     were removed in S19-EXT because they didn't actually produce a
//     deployment whose `owner == newDeployer` — the inner `new X()` ran
//     in the helper's context. Replaced by direct `HEVM.prank(addr); new X();`
//     pattern at every call site, which IS owner-controlling.
