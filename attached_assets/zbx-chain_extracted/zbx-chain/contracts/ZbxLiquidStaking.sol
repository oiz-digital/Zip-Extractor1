// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title  ZbxLiquidStaking — Liquid staking: deposit ZBX, receive stZBX
/// @author Zebvix Technologies Pvt Ltd
///
/// @notice Users deposit native ZBX and receive stZBX ERC-20 tokens.
///         stZBX is share-based: as validator rewards accrue in the pool,
///         each stZBX becomes redeemable for more ZBX over time.
///
///         Exchange rate:  1 stZBX = totalZbx / totalShares  ZBX
///
///         Example:
///           Pool starts empty. Alice deposits 100 ZBX → receives 100 stZBX.
///           Operator adds 10 ZBX rewards → pool = 110 ZBX, shares = 100.
///           1 stZBX now worth 1.1 ZBX.
///           Bob deposits 110 ZBX → receives 100 stZBX (110 / 1.1).
///           Pool = 220 ZBX, shares = 200.
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:module     DeFi / Liquid Staking (ZEP-033)

interface IZbxStakingReceiver {
    function onStakeReceived(address staker, uint256 zbxAmount) external;
}

contract ZbxLiquidStaking {

    // ─── Errors ───────────────────────────────────────────────────────────

    error ZeroAmount();
    error ZeroAddress();
    error InsufficientShares();
    error TransferFailed();
    error NotOperator();
    error NotOwner();
    error ZeroPool();
    error Paused();

    // ─── Events ───────────────────────────────────────────────────────────

    event Staked(address indexed staker, uint256 zbxIn, uint256 stZbxOut);
    event Unstaked(address indexed staker, uint256 stZbxIn, uint256 zbxOut);
    event RewardAdded(address indexed operator, uint256 amount);
    event OperatorSet(address indexed operator, bool enabled);
    event Transfer(address indexed from, address indexed to, uint256 amount);
    event Approval(address indexed owner, address indexed spender, uint256 amount);

    // ─── stZBX ERC-20 token state ─────────────────────────────────────────

    string  public constant name     = "Staked ZBX";
    string  public constant symbol   = "stZBX";
    uint8   public constant decimals = 18;

    uint256 public totalSupply;   // total stZBX shares

    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    // ─── Pool state ───────────────────────────────────────────────────────

    /// @notice Total ZBX held in the pool (deposits + rewards).
    uint256 public totalPooled;

    /// @notice Addresses authorised to add rewards (e.g., validator reward router).
    mapping(address => bool) public isOperator;

    address public owner;
    bool    public paused;

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor() {
        owner = msg.sender;
    }

    // ─── Staking ──────────────────────────────────────────────────────────

    /// @notice Deposit ZBX and receive stZBX.
    ///         Amount of stZBX = depositedZBX * totalShares / totalPooled
    ///         (or 1:1 if pool is empty).
    /// SEC-2026-05-09 Pass-15 (HIGH-S02 / Pass-12 Tier-2 LST-inflation):
    /// Permanently-locked dead-shares minted on first deposit. Pre-fix,
    /// a first depositor could mint 1 wei share, then donate a large
    /// amount directly to the contract via the `receive()` path so that
    /// `totalPooled` >> `totalSupply`. The next user's deposit of
    /// `(amount * totalSupply) / totalPooled` would round to 0 shares,
    /// effectively gifting their stake to the attacker. Locking 1000
    /// shares to address(0) at genesis raises the share denomination so
    /// the attacker would have to donate >1000× their target user's
    /// deposit to grief them — economically irrational at any scale.
    uint256 internal constant MIN_SHARES = 1_000;
    bool internal _bootstrapped;

    function stake() external payable returns (uint256 stZbxOut) {
        if (paused) revert Paused();
        uint256 zbxIn = msg.value;
        if (zbxIn == 0) revert ZeroAmount();

        if (totalSupply == 0 || totalPooled == 0) {
            require(zbxIn > MIN_SHARES, "LST: bootstrap deposit too small");
            // Burn MIN_SHARES to address(0) — these are unrecoverable
            // and bound the inflation ratio for every future depositor.
            balanceOf[address(0)] += MIN_SHARES;
            totalSupply           += MIN_SHARES;
            emit Transfer(address(0), address(0), MIN_SHARES);
            stZbxOut = zbxIn - MIN_SHARES;
            _bootstrapped = true;
        } else {
            stZbxOut = (zbxIn * totalSupply) / totalPooled;
            require(stZbxOut > 0, "LST: zero shares minted (inflation grief)");
        }

        totalPooled += zbxIn;
        totalSupply += stZbxOut;
        balanceOf[msg.sender] += stZbxOut;

        emit Transfer(address(0), msg.sender, stZbxOut);
        emit Staked(msg.sender, zbxIn, stZbxOut);
    }

    /// @notice Burn stZBX and receive ZBX back (principal + accrued rewards).
    ///         Amount of ZBX = stZbxIn * totalPooled / totalShares
    function unstake(uint256 stZbxIn) external returns (uint256 zbxOut) {
        if (stZbxIn == 0) revert ZeroAmount();
        if (balanceOf[msg.sender] < stZbxIn) revert InsufficientShares();
        if (totalSupply == 0) revert ZeroPool();

        zbxOut = (stZbxIn * totalPooled) / totalSupply;

        balanceOf[msg.sender] -= stZbxIn;
        totalSupply  -= stZbxIn;
        totalPooled  -= zbxOut;

        emit Transfer(msg.sender, address(0), stZbxIn);
        emit Unstaked(msg.sender, stZbxIn, zbxOut);

        (bool ok,) = msg.sender.call{value: zbxOut}("");
        if (!ok) revert TransferFailed();
    }

    // ─── Reward injection ─────────────────────────────────────────────────

    /// @notice Operator (validator reward router) adds ZBX rewards to pool.
    ///         This increases the stZBX exchange rate for all stakers.
    function addRewards() external payable {
        if (!isOperator[msg.sender]) revert NotOperator();
        if (msg.value == 0) revert ZeroAmount();
        totalPooled += msg.value;
        emit RewardAdded(msg.sender, msg.value);
    }

    // ─── ERC-20 functions ─────────────────────────────────────────────────

    function transfer(address to, uint256 amount) external returns (bool) {
        if (to == address(0)) revert ZeroAddress();
        if (balanceOf[msg.sender] < amount) revert InsufficientShares();
        balanceOf[msg.sender] -= amount;
        balanceOf[to]         += amount;
        emit Transfer(msg.sender, to, amount);
        return true;
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        emit Approval(msg.sender, spender, amount);
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        if (to == address(0)) revert ZeroAddress();
        if (balanceOf[from] < amount) revert InsufficientShares();
        uint256 allowed = allowance[from][msg.sender];
        if (allowed != type(uint256).max) {
            if (allowed < amount) revert InsufficientShares();
            allowance[from][msg.sender] = allowed - amount;
        }
        balanceOf[from] -= amount;
        balanceOf[to]   += amount;
        emit Transfer(from, to, amount);
        return true;
    }

    // ─── View helpers ─────────────────────────────────────────────────────

    /// @notice How much ZBX 1 stZBX is worth (18-decimal fixed point).
    function exchangeRate() external view returns (uint256) {
        if (totalSupply == 0) return 1e18;
        return (totalPooled * 1e18) / totalSupply;
    }

    /// @notice Preview: how many stZBX you'd get for `zbxAmount`.
    function previewStake(uint256 zbxAmount) external view returns (uint256) {
        if (totalSupply == 0 || totalPooled == 0) return zbxAmount;
        return (zbxAmount * totalSupply) / totalPooled;
    }

    /// @notice Preview: how much ZBX you'd get for burning `stZbxAmount`.
    function previewUnstake(uint256 stZbxAmount) external view returns (uint256) {
        if (totalSupply == 0) return 0;
        return (stZbxAmount * totalPooled) / totalSupply;
    }

    // ─── Admin ────────────────────────────────────────────────────────────

    function setOperator(address op, bool enabled) external {
        if (msg.sender != owner) revert NotOwner();
        if (op == address(0)) revert ZeroAddress();
        isOperator[op] = enabled;
        emit OperatorSet(op, enabled);
    }

    function setPaused(bool p) external {
        if (msg.sender != owner) revert NotOwner();
        paused = p;
    }

    receive() external payable {}
}
