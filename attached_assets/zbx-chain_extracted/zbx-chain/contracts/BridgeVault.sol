// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import { IZBX } from "./interfaces/IZBX.sol";
import { IBridgeVault } from "./interfaces/IBridgeVault.sol";
import { ReentrancyGuard } from "./libraries/ReentrancyGuard.sol";

/// @title BridgeVault — BSC-side lock/release vault for wrapped ZBX
/// @author Zebvix Technologies Pvt Ltd
/// @notice Two-way teleport between the BNB Chain ZBX BEP-20 token and
///         native Zebvix L1 ZBX. Uses a burn-and-emit / mint-on-quorum
///         pattern so circulating supply on BSC is always ≤ amount locked
///         in the Zebvix native bridge vault `0x7a627…0000`.
///
/// @dev    Authority chain (post-architect-review):
///             user ─approve→ vault
///             user ─lock()→ vault ─bridgeBurnFrom→ token (vault is sole burner)
///
///             relayer ─submitMint→ multisig ─executeMint→ vault
///                                          (multisig is sole executeMint caller)
///             vault ─bridgeMint(seq)→ token (vault is sole minter)
///
///         All replay protection lives on the vault; multisig only verifies
///         signatures. Reentrancy guarded with inlined OZ pattern.
contract BridgeVault is IBridgeVault, ReentrancyGuard {

    // ─── S25-Y4 unchecked policy ─────────────────────────────────────────
    // All `unchecked { ... }` blocks in this contract fall into ONE of these
    // proven-safe categories (per S25 hardening pass):
    //   (a) post-require subtraction — preceding `require(x >= y)` proves
    //       `x - y` cannot underflow (token balance debits, allowance debits).
    //   (b) conservation pair — incrementing one slot by exactly the value
    //       just decremented (or vice-versa) from another, with the totalSupply
    //       invariant pre-checked (mint/burn/transfer leg of accounting pair).
    //   (c) bounded for-loop counter — `for (i; i < len; ) { ...; unchecked
    //       { i++; } }` where `len` is the bound; standard gas-saving pattern.
    //   (d) modular wrap intentional — uint32 timestamp/sequence wrap arithmetic
    //       (Uniswap V2 style); the wrap IS the spec.
    //   (e) UQ112x112 fixed-point shift — pre-bounded by uint112 reserve
    //       invariants (Uniswap V2 oracle accumulator).
    // Reviewers MUST classify any future `unchecked` block in this file
    // against one of (a)-(e) before merging; new categories require AUDIT entry.
    // ─────────────────────────────────────────────────────────────────────
    // ---------------------------------------------------------------------
    // Immutable wiring
    // ---------------------------------------------------------------------

    address public immutable override token;     // ZRC20
    address public immutable override multisig;  // BridgeMultisig

    // ---------------------------------------------------------------------
    // Mutable state
    // ---------------------------------------------------------------------

    address public founder;
    /// @notice 2-step founder-transfer destination. The successor must
    ///         explicitly call `acceptFounder()` — single-step transfer was
    ///         removed because mistyping the new address would brick the vault.
    address public pendingFounder;
    /// @notice High-risk admin executor (typically `ZbxTimelock`). When zero,
    ///         the founder may still call `onlyAdmin` functions for bootstrap;
    ///         once set, those functions revert unless `msg.sender == governor`.
    address public governor;
    bool    public override paused;

    /// @notice Monotonic vault-side sequence number for `Locked` events.
    ///         Combined with `block.chainid` it produces a globally-unique
    ///         `source_tx_hash` for Zebvix replay protection.
    uint64 public override nextSeq = 1;

    /// @notice Outstanding wrapped ZBX in circulation = mints - burns.
    ///         Always equal to `IZBX(token).totalSupply()` if invariants hold.
    uint256 public override totalLocked;

    /// @notice Replay-protection set: `executeMint` may only credit each
    ///         Zebvix outbound sequence once.
    mapping(uint64 => bool) private _processedZebvixSeq;

    // ---------------------------------------------------------------------
    // Mint rate-limit (defense-in-depth against multisig compromise)
    // ---------------------------------------------------------------------

    /// @notice Maximum tokens that may be minted by `executeMint` within a
    ///         24-hour TUMBLING window. Founder-configurable. A value of 0
    ///         disables the cap entirely (e.g. during initial bootstrap).
    /// @dev    Audit-2026-05-01 S6-BV1: the previous comment said "rolling".
    ///         The implementation is tumbling — once block.timestamp crosses
    ///         `mintWindowStart + MINT_WINDOW`, the bucket resets to 0 and
    ///         the full cap is available again. Burst behaviour: an attacker
    ///         can mint full cap at T = window_end - 1s and again at
    ///         T = window_end + 1s, getting 2× cap in 2 seconds. Treat this
    ///         as the documented worst case when sizing dailyMintCap.
    ///         A true rolling cap requires per-mint timestamp queue (more
    ///         complex; tracked as P3 in AUDIT_2026-04-30.md S6-BV1).
    uint256 public dailyMintCap;

    /// @notice Maximum tokens mintable in a single `executeMint` call.
    ///         Independent of the rolling cap — guards against a single
    ///         catastrophic forged proof.
    uint256 public perTxMintCap;

    /// @notice Sliding-window state: amount minted in the current 24h bucket.
    uint256 public mintedInWindow;

    /// @notice Timestamp at which the current mint window started.
    uint40  public mintWindowStart;

    uint256 public constant MINT_WINDOW = 1 days;

    event MintCapsUpdated(uint256 dailyCap, uint256 perTxCap);
    event MintWindowReset(uint40 newStart);
    error DailyMintCapExceeded(uint256 attempted, uint256 remaining);
    error PerTxMintCapExceeded(uint256 attempted, uint256 cap);

    // ---------------------------------------------------------------------
    // Reentrancy guard
    // ---------------------------------------------------------------------

    // SEC-2026-05-09: migrated to libraries/ReentrancyGuard.sol — single
    // shared audit surface across the codebase. Modifier `nonReentrant`
    // (and the `_status` storage slot) are inherited from the base.

    // ---------------------------------------------------------------------
    // Errors
    // ---------------------------------------------------------------------

    error NotMultisig();
    error NotFounder();
    error NotPendingFounder();
    error NotAdmin();
    error PausedErr();
    error ZeroAmount();
    error InvalidDest();
    error AlreadyProcessed(uint64 zebvixSeq);
    error TransferFailed();
    error InsufficientLocked(uint256 amount, uint256 totalLockedNow);

    // ---------------------------------------------------------------------
    // Events (on top of IBridgeVault's)
    // ---------------------------------------------------------------------

    event FounderTransferStarted(address indexed currentFounder, address indexed pendingFounder);
    event FounderTransferred(address indexed from, address indexed to);
    event GovernorSet(address indexed previousGovernor, address indexed newGovernor);
    event Recovered(address indexed token, address indexed to, uint256 amount);

    // ---------------------------------------------------------------------
    // Constructor
    // ---------------------------------------------------------------------

    constructor(address _token, address _multisig, address _founder) {
        require(_token != address(0) && _multisig != address(0) && _founder != address(0),
                "ZERO_ADDRESS");
        token    = _token;
        multisig = _multisig;
        founder  = _founder;
    }

    // ---------------------------------------------------------------------
    // Modifiers
    // ---------------------------------------------------------------------

    modifier onlyMultisig() {
        if (msg.sender != multisig) revert NotMultisig();
        _;
    }

    modifier onlyFounder() {
        if (msg.sender != founder) revert NotFounder();
        _;
    }

    /// @notice High-risk admin gate. Routes to the timelock once
    ///         `setGovernor(timelock)` is wired; otherwise falls back to the
    ///         bootstrap founder. Direct founder calls AFTER governance is
    ///         enabled revert — exactly the property a production bridge needs.
    modifier onlyAdmin() {
        if (governor != address(0)) {
            if (msg.sender != governor) revert NotAdmin();
        } else {
            if (msg.sender != founder) revert NotAdmin();
        }
        _;
    }

    modifier whenNotPaused() {
        if (paused) revert PausedErr();
        _;
    }

    // ---------------------------------------------------------------------
    // BSC → Zebvix (lock + emit)
    // ---------------------------------------------------------------------

    /// @inheritdoc IBridgeVault
    function lock(uint256 amount, bytes calldata zebvixDest)
        external
        override
        nonReentrant
        whenNotPaused
        returns (uint64 seq)
    {
        if (amount == 0) revert ZeroAmount();
        if (zebvixDest.length != 20) revert InvalidDest();
        if (amount > totalLocked) revert InsufficientLocked(amount, totalLocked);

        // Burn from caller — caller must have approved this vault on the
        // token contract for at least `amount`. Burn shrinks BSC supply.
        IZBX(token).bridgeBurnFrom(msg.sender, amount, zebvixDest);

        seq = nextSeq++;
        unchecked {
            // Burn just reduced totalSupply on the token; mirror it locally
            // so `totalLocked` stays equal to `token.totalSupply()`.
            totalLocked -= amount;
        }

        emit Locked(msg.sender, amount, zebvixDest, seq);
    }

    /// @inheritdoc IBridgeVault
    function lockWithPermit(
        uint256 amount,
        bytes calldata zebvixDest,
        uint256 deadline,
        uint8 v,
        bytes32 r,
        bytes32 s
    ) external override nonReentrant whenNotPaused returns (uint64 seq) {
        if (amount == 0) revert ZeroAmount();
        if (zebvixDest.length != 20) revert InvalidDest();
        if (amount > totalLocked) revert InsufficientLocked(amount, totalLocked);

        // EIP-2612 permit lets us skip the separate approve() tx.
        // Use low-level call so IZBX doesn't need to expose permit().
        (bool ok, ) = token.call(
            abi.encodeWithSignature(
                "permit(address,address,uint256,uint256,uint8,bytes32,bytes32)",
                msg.sender,
                address(this),
                amount,
                deadline,
                v, r, s
            )
        );
        if (!ok) revert TransferFailed();

        IZBX(token).bridgeBurnFrom(msg.sender, amount, zebvixDest);

        seq = nextSeq++;
        unchecked {
            totalLocked -= amount;
        }

        emit Locked(msg.sender, amount, zebvixDest, seq);
    }

    // ---------------------------------------------------------------------
    // Zebvix → BSC (multisig-gated mint)
    // ---------------------------------------------------------------------

    /// @inheritdoc IBridgeVault
    function executeMint(address to, uint256 amount, uint64 zebvixSeq)
        external
        override
        onlyMultisig
        nonReentrant
        whenNotPaused
    {
        if (amount == 0) revert ZeroAmount();
        if (to == address(0)) revert InvalidDest();
        if (_processedZebvixSeq[zebvixSeq]) revert AlreadyProcessed(zebvixSeq);

        // ── Per-tx cap ───────────────────────────────────────────────
        if (perTxMintCap != 0 && amount > perTxMintCap) {
            revert PerTxMintCapExceeded(amount, perTxMintCap);
        }

        // ── Rolling-window daily cap ─────────────────────────────────
        if (dailyMintCap != 0) {
            // Reset window if more than MINT_WINDOW elapsed.
            if (block.timestamp >= mintWindowStart + MINT_WINDOW) {
                mintWindowStart = uint40(block.timestamp);
                mintedInWindow  = 0;
                emit MintWindowReset(mintWindowStart);
            }
            uint256 remaining = dailyMintCap > mintedInWindow
                ? dailyMintCap - mintedInWindow : 0;
            if (amount > remaining) {
                revert DailyMintCapExceeded(amount, remaining);
            }
            unchecked { mintedInWindow += amount; }
        }

        _processedZebvixSeq[zebvixSeq] = true;

        unchecked {
            totalLocked += amount;
        }

        // Vault is the sole minter on the token (set via setVault → lockVault).
        IZBX(token).bridgeMint(to, amount, zebvixSeq);

        emit Minted(to, amount, zebvixSeq);
    }

    // ---------------------------------------------------------------------
    // Read helpers
    // ---------------------------------------------------------------------

    /// @inheritdoc IBridgeVault
    function isZebvixSeqProcessed(uint64 zebvixSeq)
        external
        view
        override
        returns (bool)
    {
        return _processedZebvixSeq[zebvixSeq];
    }

    // ---------------------------------------------------------------------
    // Founder ops
    // ---------------------------------------------------------------------

    function setPaused(bool _p) external onlyFounder {
        paused = _p;
        emit PausedSet(_p);
    }

    /// @notice Configure mint rate limits. Set both to 0 to disable.
    /// @dev    Recommended values for ZBX (18 dec): dailyCap = 1_000_000e18,
    ///         perTxCap = 100_000e18. Tighten over time as bridge matures.
    ///         Gated by `onlyAdmin` because raising the cap is the exact
    ///         knob a stolen founder key would turn to abuse minting.
    function setMintCaps(uint256 _dailyCap, uint256 _perTxCap)
        external
        onlyAdmin
    {
        dailyMintCap = _dailyCap;
        perTxMintCap = _perTxCap;
        emit MintCapsUpdated(_dailyCap, _perTxCap);
    }

    /// @notice Step 1 of 2-step founder transfer. Nominate a successor;
    ///         they must call `acceptFounder()` to activate. Setting
    ///         `address(0)` cancels any pending transfer.
    function transferFounder(address newFounder) external onlyFounder {
        pendingFounder = newFounder;
        emit FounderTransferStarted(founder, newFounder);
    }

    /// @notice Step 2 of 2-step founder transfer.
    function acceptFounder() external {
        if (msg.sender != pendingFounder) revert NotPendingFounder();
        emit FounderTransferred(founder, pendingFounder);
        founder        = pendingFounder;
        pendingFounder = address(0);
    }

    /// @notice Hand high-risk admin control to a timelock / governance
    ///         executor. Gated by `onlyAdmin` so the cutover is one-way
    ///         from the founder's perspective:
    ///
    ///         - **Pre-cutover** (`governor == 0`): founder bootstraps the
    ///           timelock address.
    ///         - **Post-cutover** (`governor != 0`): only the current
    ///           governor (the timelock) may change `governor`. A stolen
    ///           founder key cannot re-point the governor to an attacker EOA
    ///           and immediately drain via `setMintCaps` / `recoverStray` —
    ///           every change must itself wait the timelock delay.
    function setGovernor(address newGovernor) external onlyAdmin {
        emit GovernorSet(governor, newGovernor);
        governor = newGovernor;
    }

    /// @notice Sweep stray tokens (not ZBX) accidentally sent to the vault.
    ///         Cannot be used to drain ZBX — that would break the bridge
    ///         invariant. Routed through the timelock once governance is
    ///         enabled (an attacker with the founder key alone cannot drain).
    function recoverStray(address strayToken, address to, uint256 amount)
        external
        onlyAdmin
    {
        require(strayToken != token, "CANNOT_RECOVER_ZBX");
        require(to != address(0), "ZERO_ADDRESS");

        (bool ok, bytes memory data) = strayToken.call(
            abi.encodeWithSignature("transfer(address,uint256)", to, amount)
        );
        require(ok && (data.length == 0 || abi.decode(data, (bool))), "RECOVERY_FAILED");
        emit Recovered(strayToken, to, amount);
    }
}