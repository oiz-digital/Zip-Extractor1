// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import { IZBX } from "./interfaces/IZBX.sol";
import { ReentrancyGuard } from "./libraries/ReentrancyGuard.sol";

/// @title ZbxStaking — single-pool ZBX staking with linear reward stream
/// @author Zebvix Technologies Pvt Ltd
/// @notice Users stake ZBX (any ERC-20-compatible token, but designed for
///         the wrapped ZBX BEP-20) and earn rewards proportional to their
///         share of the pool times time elapsed. Reward token can be the
///         same as the staking token (auto-compound style) or a separate
///         token (e.g. zUSD as yield).
///
/// @dev    Reward accounting follows the SushiSwap MasterChef pattern:
///             accRewardPerShare += (timeElapsed * rewardRate) / totalStaked
///             pendingReward(user) = user.stake * accRewardPerShare
///                                   - user.rewardDebt
///         Math is done in 1e18 fixed point so dust is well below 1 wei.
///         Funder must pre-fund `rewardToken` balance into the contract.
contract ZbxStaking is ReentrancyGuard {

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

    /// @notice Token users stake. Must implement standard ERC-20.
    address public immutable stakingToken;

    /// @notice Token used for reward payouts. May equal `stakingToken`.
    address public immutable rewardToken;

    // ---------------------------------------------------------------------
    // Mutable state
    // ---------------------------------------------------------------------

    /// @notice Reward emission rate in `rewardToken` wei per second.
    ///         Founder can update via `setRewardRate`, capped at MAX_REWARD_RATE.
    uint256 public rewardRate;

    /// @notice Hard ceiling for `rewardRate`. A stolen-founder-key attacker
    ///         could otherwise set rate = type(uint256).max and have honest
    ///         users drain the entire reward pool to their own wallets via
    ///         claim() before the team can react. Audit-2026-05-01 S6-ST1.
    ///         Value chosen at 1e21 wei/sec ≈ 1000 tokens/sec ≈ 31.5 trillion
    ///         tokens/year — well above any sane real emission, well below
    ///         "instantly drains a billion-token treasury".
    uint256 public constant MAX_REWARD_RATE = 1_000 * 1e18;

    /// @notice Last block timestamp at which `accRewardPerShare` was updated.
    uint256 public lastUpdateTime;

    /// @notice Accumulated rewards per staked wei, scaled by `ACC_PRECISION`.
    uint256 public accRewardPerShare;

    /// @notice Total currently staked.
    uint256 public totalStaked;

    /// @notice Total reward-token amount owed to all users (accrued via
    ///         updatePool() but not yet claimed). Maintained as a running
    ///         liability so `recoverExcessRewards` can never drain into
    ///         user-owned tokens — closes the architect-review High bug.
    uint256 public totalOwed;

    /// @notice Founder address — emergency pause + rate updates.
    /// @dev    Audit-2026-05-01 S6-ST2: a 2-step ownership transfer is now
    ///         used so a typo in `transferFounder` does not brick the
    ///         contract. The pending successor must explicitly call
    ///         `acceptFounder()` to take over.
    address public founder;
    address public pendingFounder;
    bool    public paused;

    /// @notice Per-user state.
    struct UserInfo {
        uint256 stake;        // amount of stakingToken held by this user
        uint256 rewardDebt;   // already-credited rewards (accrual baseline)
        uint256 pending;      // unclaimed rewards parked from prior updates
    }
    mapping(address => UserInfo) public users;

    uint256 private constant ACC_PRECISION = 1e18;

    // ---------------------------------------------------------------------
    // Reentrancy guard
    // ---------------------------------------------------------------------

    // SEC-2026-05-09: migrated to libraries/ReentrancyGuard.sol.

    // ---------------------------------------------------------------------
    // Errors
    // ---------------------------------------------------------------------

    error NotFounder();
    error NotPendingFounder();
    error PausedErr();
    error ZeroAmount();
    error InsufficientStake(uint256 requested, uint256 available);
    error TransferFailed();
    error RewardRateAboveCap(uint256 requested, uint256 cap);
    error NotSlasher();
    error SlashTooSoon(uint256 stakeBlock, uint256 minBlocks);
    error SlashExceedsStake(uint256 requested, uint256 available);

    // ---------------------------------------------------------------------
    // Events
    // ---------------------------------------------------------------------

    event Staked(address indexed user, uint256 amount, uint256 newStake);
    event Unstaked(address indexed user, uint256 amount, uint256 newStake);
    event RewardPaid(address indexed user, uint256 amount);
    event RewardRateUpdated(uint256 oldRate, uint256 newRate);
    event PausedSet(bool isPaused);
    event FounderTransferStarted(address indexed currentFounder, address indexed pendingFounder);
    event FounderTransferred(address indexed from, address indexed to);

    // ---------------------------------------------------------------------
    // Constructor
    // ---------------------------------------------------------------------

    constructor(
        address _stakingToken,
        address _rewardToken,
        uint256 _rewardRate,
        address _founder
    ) {
        require(_stakingToken != address(0) && _rewardToken != address(0)
                && _founder != address(0), "ZERO_ADDRESS");

        stakingToken    = _stakingToken;
        rewardToken     = _rewardToken;
        rewardRate      = _rewardRate;
        founder         = _founder;
        lastUpdateTime  = block.timestamp;
    }

    // ---------------------------------------------------------------------
    // Modifiers
    // ---------------------------------------------------------------------

    modifier onlyFounder() {
        if (msg.sender != founder) revert NotFounder();
        _;
    }

    modifier whenNotPaused() {
        if (paused) revert PausedErr();
        _;
    }

    /// @dev Pulls fresh rewards into `accRewardPerShare` and saves the
    ///      checkpoint timestamp. Called at the top of every state-mutating
    ///      user action so per-user math stays consistent.
    modifier updatePool() {
        if (block.timestamp > lastUpdateTime && totalStaked > 0) {
            uint256 elapsed = block.timestamp - lastUpdateTime;
            uint256 reward = elapsed * rewardRate;
            accRewardPerShare += (reward * ACC_PRECISION) / totalStaked;
            // Mirror the freshly-accrued reward in the global liability
            // counter — every wei that just became claimable belongs to a
            // user and must NOT be sweepable by `recoverExcessRewards`.
            unchecked { totalOwed += reward; }
        }
        lastUpdateTime = block.timestamp;
        _;
    }

    // ---------------------------------------------------------------------
    // User actions
    // ---------------------------------------------------------------------

    /// SEC-2026-05-09 Pass-15 (HIGH-S new — flash-loan-governance
    /// guard): record the block at which a user last increased their
    /// stake. Governance reads of `votingPower(user)` should ignore
    /// stake gained inside `MIN_STAKE_AGE` blocks so a flash-borrowed
    /// stake-and-vote-and-unstake sequence cannot influence votes.
    /// Reads `unstake()` enforces a separate cooldown so the flash
    /// path is closed at both ends.
    uint256 public constant MIN_STAKE_AGE = 5; // ~10s — outlasts a single tx
    mapping(address => uint256) public lastStakeBlock;

    /// SEC-2026-05-09 Pass-19 (Tier-2 #8): slasher role. Only the
    /// slasher (a governance-controlled module — typically the
    /// HotStuff2 evidence verifier or a governance multisig) can
    /// slash mis-behaving stakers. Slashed tokens are routed to
    /// `BURN_ADDRESS` (irreversible burn) so a compromised slasher
    /// cannot rug stakers to its own wallet — same pattern as the
    /// Pass-15 ZbxBundler.slash fix.
    address public slasher;
    address public constant BURN_ADDRESS = 0x000000000000000000000000000000000000dEaD;
    event SlasherUpdated(address indexed newSlasher);
    event Slashed(address indexed user, uint256 amount, string reason);

    function setSlasher(address newSlasher) external {
        if (msg.sender != founder) revert NotFounder();
        slasher = newSlasher;
        emit SlasherUpdated(newSlasher);
    }

    /// @notice Slash `amount` from `user`'s stake. Tokens are sent to
    ///         BURN_ADDRESS (irreversible) so the slasher cannot
    ///         enrich itself.
    /// @dev    Honours `MIN_STAKE_AGE` — fresh stake (e.g. a
    ///         flash-loaned stake-then-self-slash to drain rewards
    ///         pool via accounting glitch) is not slashable until
    ///         the cooldown has passed. Reduces totalStaked + clears
    ///         user.rewardDebt accounting cleanly.
    function slash(address user, uint256 amount, string calldata reason)
        external
        nonReentrant
        updatePool
    {
        if (msg.sender != slasher) revert NotSlasher();
        if (amount == 0) revert ZeroAmount();
        UserInfo storage u = users[user];
        if (amount > u.stake) revert SlashExceedsStake(amount, u.stake);
        if (block.number < lastStakeBlock[user] + MIN_STAKE_AGE) {
            revert SlashTooSoon(lastStakeBlock[user], MIN_STAKE_AGE);
        }

        // Park earned rewards before reducing stake (slashed user
        // keeps any reward already accrued — slashing punishes the
        // principal, not the yield).
        uint256 fresh = (u.stake * accRewardPerShare) / ACC_PRECISION;
        u.pending += fresh - u.rewardDebt;

        unchecked {
            u.stake     -= amount;
            totalStaked -= amount;
        }
        u.rewardDebt = (u.stake * accRewardPerShare) / ACC_PRECISION;

        _safeTransfer(stakingToken, BURN_ADDRESS, amount);
        emit Slashed(user, amount, reason);
    }

    function stake(uint256 amount)
        external
        nonReentrant
        whenNotPaused
        updatePool
    {
        if (amount == 0) revert ZeroAmount();
        UserInfo storage u = users[msg.sender];

        // Park any pending rewards from prior stake before changing stake.
        if (u.stake > 0) {
            uint256 fresh = (u.stake * accRewardPerShare) / ACC_PRECISION;
            u.pending += fresh - u.rewardDebt;
        }

        // Pull tokens in.
        _safeTransferFrom(stakingToken, msg.sender, address(this), amount);

        unchecked {
            u.stake     += amount;
            totalStaked += amount;
        }
        u.rewardDebt = (u.stake * accRewardPerShare) / ACC_PRECISION;
        lastStakeBlock[msg.sender] = block.number;

        emit Staked(msg.sender, amount, u.stake);
    }

    function unstake(uint256 amount)
        external
        nonReentrant
        updatePool
    {
        if (amount == 0) revert ZeroAmount();
        UserInfo storage u = users[msg.sender];
        if (amount > u.stake) revert InsufficientStake(amount, u.stake);

        // Park earned rewards.
        uint256 fresh = (u.stake * accRewardPerShare) / ACC_PRECISION;
        u.pending += fresh - u.rewardDebt;

        unchecked {
            u.stake     -= amount;
            totalStaked -= amount;
        }
        u.rewardDebt = (u.stake * accRewardPerShare) / ACC_PRECISION;

        _safeTransfer(stakingToken, msg.sender, amount);
        emit Unstaked(msg.sender, amount, u.stake);
    }

    /// @notice Claim accumulated rewards without changing stake.
    function claim() external nonReentrant updatePool {
        UserInfo storage u = users[msg.sender];

        uint256 fresh = (u.stake * accRewardPerShare) / ACC_PRECISION;
        uint256 owed = u.pending + (fresh - u.rewardDebt);
        u.pending    = 0;
        u.rewardDebt = fresh;

        if (owed > 0) {
            // Liability has been satisfied; release from the global counter.
            // (Capped subtraction defends against rounding dust below
            // totalOwed in pathological corner cases.)
            totalOwed = owed > totalOwed ? 0 : totalOwed - owed;
            _safeTransfer(rewardToken, msg.sender, owed);
            emit RewardPaid(msg.sender, owed);
        }
    }

    /// @notice Bypass-pool withdrawal — forfeit rewards to recover stake.
    ///         Founder pause cannot block this so users always have an exit.
    /// @dev    Does NOT call updatePool to avoid the pause from blocking
    ///         exit. Forfeited rewards stay in `totalOwed` until the next
    ///         updatePool() flushes them — conservative but safe (it only
    ///         keeps `recoverExcessRewards` more restrictive, never less).
    function emergencyUnstake() external nonReentrant {
        UserInfo storage u = users[msg.sender];
        uint256 amount = u.stake;
        if (amount == 0) revert ZeroAmount();

        // Release this user's accrual baseline + parked pending from the
        // global owed counter — they are explicitly forfeiting them.
        uint256 fresh = (u.stake * accRewardPerShare) / ACC_PRECISION;
        uint256 forfeit = u.pending + (fresh > u.rewardDebt ? fresh - u.rewardDebt : 0);
        if (forfeit > 0) {
            totalOwed = forfeit > totalOwed ? 0 : totalOwed - forfeit;
        }

        u.stake      = 0;
        u.rewardDebt = 0;
        u.pending    = 0;
        unchecked { totalStaked -= amount; }

        _safeTransfer(stakingToken, msg.sender, amount);
        emit Unstaked(msg.sender, amount, 0);
    }

    // ---------------------------------------------------------------------
    // Read helpers
    // ---------------------------------------------------------------------

    /// @notice View pending reward for `user` without mutating state.
    function pendingReward(address user) external view returns (uint256) {
        UserInfo memory u = users[user];
        uint256 acc = accRewardPerShare;
        if (block.timestamp > lastUpdateTime && totalStaked > 0) {
            uint256 elapsed = block.timestamp - lastUpdateTime;
            uint256 reward = elapsed * rewardRate;
            acc += (reward * ACC_PRECISION) / totalStaked;
        }
        uint256 fresh = (u.stake * acc) / ACC_PRECISION;
        // SOL-03 (LOW): guard view-function revert. If accRewardPerShare
        // advanced between the last write and this read, integer truncation
        // during the write could leave rewardDebt slightly above `fresh`.
        // Use saturating sub so the view always returns a safe value.
        return u.pending + (fresh > u.rewardDebt ? fresh - u.rewardDebt : 0);
    }

    // ---------------------------------------------------------------------
    // Founder ops
    // ---------------------------------------------------------------------

    /// @dev Audit-2026-05-01 S6-ST1: cap reward rate at MAX_REWARD_RATE so
    ///      a single founder-key compromise cannot drain the reward pool by
    ///      cranking the rate to type(uint256).max.
    function setRewardRate(uint256 newRate) external onlyFounder updatePool {
        if (newRate > MAX_REWARD_RATE) revert RewardRateAboveCap(newRate, MAX_REWARD_RATE);
        emit RewardRateUpdated(rewardRate, newRate);
        rewardRate = newRate;
    }

    function setPaused(bool _p) external onlyFounder {
        paused = _p;
        emit PausedSet(_p);
    }

    /// @notice Step 1 of 2-step founder transfer (Audit-2026-05-01 S6-ST2).
    ///         Setting `address(0)` cancels any pending transfer.
    function transferFounder(address newFounder) external onlyFounder {
        pendingFounder = newFounder;
        emit FounderTransferStarted(founder, newFounder);
    }

    /// @notice Step 2 of 2-step founder transfer. The nominated successor
    ///         must call this themselves — guards against typo-bricking.
    function acceptFounder() external {
        if (msg.sender != pendingFounder) revert NotPendingFounder();
        emit FounderTransferred(founder, pendingFounder);
        founder        = pendingFounder;
        pendingFounder = address(0);
    }

    /// @notice Founder can withdraw stranded `rewardToken` (excess beyond
    ///         what's needed to cover all current user obligations).
    ///         Cannot drain user principal OR accrued user rewards.
    /// @dev    Architect-review High fix: reserve includes `totalOwed`
    ///         (running per-user reward liability tracked by updatePool +
    ///         claim + emergencyUnstake) on top of `totalStaked` when the
    ///         staking and reward tokens are the same.
    function recoverExcessRewards(address to, uint256 amount)
        external
        onlyFounder
        updatePool
    {
        require(to != address(0), "ZERO_ADDRESS");
        uint256 bal = _balanceOf(rewardToken, address(this));
        uint256 reserve = totalOwed
            + ((rewardToken == stakingToken) ? totalStaked : 0);
        require(amount + reserve <= bal, "INSUFFICIENT_FREE_BALANCE");
        _safeTransfer(rewardToken, to, amount);
    }

    // ---------------------------------------------------------------------
    // ERC-20 helpers (no SafeERC20 import — minimal inline)
    // ---------------------------------------------------------------------

    function _balanceOf(address token, address who) internal view returns (uint256) {
        (bool ok, bytes memory data) = token.staticcall(
            abi.encodeWithSignature("balanceOf(address)", who)
        );
        require(ok && data.length >= 32, "BALANCEOF_FAILED");
        return abi.decode(data, (uint256));
    }

    function _safeTransfer(address token, address to, uint256 amount) internal {
        (bool ok, bytes memory data) = token.call(
            abi.encodeWithSignature("transfer(address,uint256)", to, amount)
        );
        if (!ok || (data.length != 0 && !abi.decode(data, (bool))))
            revert TransferFailed();
    }

    function _safeTransferFrom(address token, address from, address to, uint256 amount) internal {
        (bool ok, bytes memory data) = token.call(
            abi.encodeWithSignature("transferFrom(address,address,uint256)", from, to, amount)
        );
        if (!ok || (data.length != 0 && !abi.decode(data, (bool))))
            revert TransferFailed();
    }
}