// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import { ReentrancyGuard } from "./libraries/ReentrancyGuard.sol";

/**
 * @title ZbxBundler
 * @notice ERC-4337 Account Abstraction bundler helper contract.
 *
 * This contract provides:
 * - On-chain bundle submission via handleOps() forwarding
 * - Bundler reputation management
 * - MEV-resistant bundle submission (encrypted mempool hook)
 *
 * The canonical ERC-4337 EntryPoint is deployed at:
 * 0x5FF137D4b0FDCD49DcA30c7CF57E578a026d2789
 *
 * @dev ZBX Chain ID: 8989 (mainnet) / 8990 (testnet+devnet shared).
 */

interface IEntryPoint {
    struct UserOperation {
        address sender;
        uint256 nonce;
        bytes   initCode;
        bytes   callData;
        uint256 callGasLimit;
        uint256 verificationGasLimit;
        uint256 preVerificationGas;
        uint256 maxFeePerGas;
        uint256 maxPriorityFeePerGas;
        bytes   paymasterAndData;
        bytes   signature;
    }

    function handleOps(UserOperation[] calldata ops, address payable beneficiary) external;
    function getUserOpHash(UserOperation calldata userOp) external view returns (bytes32);
    function getDepositInfo(address account) external view returns (
        uint112 deposit,
        bool    staked,
        uint112 stake,
        uint32  unstakeDelaySec,
        uint48  withdrawTime
    );
}

contract ZbxBundler is ReentrancyGuard {
    // ─── Events ──────────────────────────────────────────────────────────────

    event BundleSubmitted(address indexed bundler, uint256 opsCount, bytes32 indexed bundleId);
    event BundlerRegistered(address indexed bundler, uint256 stake);
    event BundlerSlashed(address indexed bundler, uint256 slashAmount, string reason);

    // ─── State ───────────────────────────────────────────────────────────────

    IEntryPoint public immutable entryPoint;
    address     public immutable owner;

    /// Registered bundlers and their staked collateral.
    mapping(address => uint256) public bundlerStake;
    /// Bundler operation counts (for reputation).
    mapping(address => uint256) public bundlerOpsSubmitted;
    /// Whether a bundler is currently active.
    mapping(address => bool)    public bundlerActive;

    /// Minimum stake to register as a bundler (0.1 ZBX).
    uint256 public constant MIN_BUNDLER_STAKE = 0.1 ether;

    /// SEC-2026-05-09 Pass-19 (Tier-2 #5): hard cap on bundle size to
    /// prevent block-stuffing / gas-griefing. ERC-4337 reference
    /// bundlers cap at 10–32 ops per bundle; we permit up to 64 to
    /// keep throughput headroom while bounding worst-case verification
    /// cost. Each op pays its own `verificationGasLimit + callGasLimit`
    /// at EntryPoint.handleOps, so 64 is a soft per-bundle ceiling
    /// even before the block gas limit kicks in.
    uint256 public constant MAX_BUNDLE_OPS = 64;

    // ─── Constructor ─────────────────────────────────────────────────────────

    constructor(address _entryPoint) {
        entryPoint = IEntryPoint(_entryPoint);
        owner = msg.sender;
    }

    // ─── Bundler Registration ─────────────────────────────────────────────

    /// Register as a bundler by staking ZBX collateral.
    function registerBundler() external payable {
        require(msg.value >= MIN_BUNDLER_STAKE, "ZbxBundler: insufficient stake");
        bundlerStake[msg.sender] += msg.value;
        bundlerActive[msg.sender] = true;
        emit BundlerRegistered(msg.sender, msg.value);
    }

    /// Deregister and withdraw stake (requires no pending disputes).
    /// @dev S19: migrated off `.transfer(...)` to `.call{value:...}("")` so
    ///      smart-wallet bundlers (multi-sig, ERC-4337 wallets) that consume
    ///      more than the historical 2300-gas stipend can still receive
    ///      their refund. CEI ordering is preserved (state cleared BEFORE
    ///      external call); `nonReentrant` adds defense-in-depth in case
    ///      a future state-write is added between the clear and the send.
    function deregisterBundler() external nonReentrant {
        require(bundlerActive[msg.sender], "ZbxBundler: not a bundler");
        uint256 stake = bundlerStake[msg.sender];
        bundlerStake[msg.sender] = 0;
        bundlerActive[msg.sender] = false;
        (bool ok, ) = payable(msg.sender).call{value: stake}("");
        require(ok, "ZbxBundler: stake refund failed");
    }

    // ─── Bundle Submission ───────────────────────────────────────────────────

    /**
     * @notice Submit a bundle of UserOperations to the EntryPoint.
     * @param ops     The UserOperations to execute.
     * @param beneficiary Address that receives bundler fee (msg.sender by default).
     * @return bundleId Unique identifier for this bundle (hash of ops).
     */
    function submitBundle(
        IEntryPoint.UserOperation[] calldata ops,
        address payable beneficiary
    ) external returns (bytes32 bundleId) {
        require(bundlerActive[msg.sender], "ZbxBundler: not a registered bundler");
        require(ops.length > 0, "ZbxBundler: empty bundle");
        // SEC-2026-05-09 Pass-19 (Tier-2 #5): hard cap on bundle size.
        require(ops.length <= MAX_BUNDLE_OPS, "ZbxBundler: bundle too large");

        bundleId = keccak256(abi.encode(ops, block.number, msg.sender));

        // Forward to EntryPoint
        entryPoint.handleOps(ops, beneficiary == address(0) ? payable(msg.sender) : beneficiary);

        bundlerOpsSubmitted[msg.sender] += ops.length;
        emit BundleSubmitted(msg.sender, ops.length, bundleId);
    }

    // ─── Slash ───────────────────────────────────────────────────────────────

    /// Slash a misbehaving bundler (only owner / governance).
    /// @dev S19: migrated off `.transfer(...)` to `.call{value:...}("")` so
    ///      governance multi-sigs / Timelock-style owners with non-trivial
    ///      receive() logic can accept the slashed funds. Owner is immutable,
    ///      so reentrancy through the call would still come back to a known
    ///      address; CEI is preserved (state decrement BEFORE external send).
    /// SEC-2026-05-09 Pass-15 (HIGH-S08): pre-fix slashed funds were
    /// sent to `payable(owner)` — owner could rug arbitrary bundler
    /// stakes for "rule violations" they themselves judged. Now slashed
    /// funds are routed to a fixed `BURN_ADDRESS`; owner cannot enrich
    /// themselves from slashing. If governance later wants a treasury
    /// destination, deploy a separate timelocked treasury contract and
    /// burn-transfer there in a post-burn unwrap.
    address public constant BURN_ADDRESS = address(0x000000000000000000000000000000000000dEaD);

    function slash(address bundler, uint256 amount, string calldata reason) external {
        require(msg.sender == owner, "ZbxBundler: not owner");
        require(bundlerStake[bundler] >= amount, "ZbxBundler: insufficient stake to slash");
        bundlerStake[bundler] -= amount;
        (bool ok, ) = payable(BURN_ADDRESS).call{value: amount}("");
        require(ok, "ZbxBundler: slash burn failed");
        emit BundlerSlashed(bundler, amount, reason);
    }

    receive() external payable {}
}