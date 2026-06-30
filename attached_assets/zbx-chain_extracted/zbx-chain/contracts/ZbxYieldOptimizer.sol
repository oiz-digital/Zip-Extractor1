// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title  ZbxYieldOptimizer — Auto-compounding yield vault
/// @author Zebvix Technologies Pvt Ltd
///
/// @notice Users deposit an asset (ERC-20 LP token or single token) into a
///         strategy vault.  A keeper calls compound() to claim rewards, swap
///         them back into the underlying asset, and re-deposit — compounding
///         returns automatically.
///
///         Users receive vault shares.  As the underlying grows, each share
///         is worth more of the underlying.
///
///         Fees:
///           - Performance fee (default 10%) on each compound on profits.
///           - Withdrawal fee (default 0.1%) to prevent sandwich attacks.
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:module     DeFi / Yield Optimizer (ZEP-035)

import { ReentrancyGuard } from "./libraries/ReentrancyGuard.sol";

interface IFarm {
    function deposit(uint256 amount) external;
    function withdraw(uint256 amount) external;
    function pendingReward(address user) external view returns (uint256);
    function claim() external;
}

interface ISwapRouter {
    function swapExactTokensForTokens(
        uint256 amountIn, uint256 amountOutMin,
        address[] calldata path, address to, uint256 deadline
    ) external returns (uint256[] memory amounts);
}

interface IERC20Opt {
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function transfer(address to, uint256 amount) external returns (bool);
    function approve(address spender, uint256 amount) external returns (bool);
    function balanceOf(address account) external view returns (uint256);
}

contract ZbxYieldOptimizer is ReentrancyGuard {

    // ─── Errors ───────────────────────────────────────────────────────────

    error ZeroAmount();
    error ZeroAddress();
    error NotKeeper();
    error NotOwner();
    error InsufficientShares();
    error FeeTooHigh();

    // ─── Events ───────────────────────────────────────────────────────────

    event Deposited(address indexed user, uint256 assets, uint256 shares);
    event Withdrawn(address indexed user, uint256 shares, uint256 assets);
    event Compounded(uint256 rewardsClaimed, uint256 assetsAdded, uint256 fee);
    event KeeperSet(address indexed keeper, bool enabled);
    event PerformanceFeeUpdated(uint256 feeBps);
    event WithdrawalFeeUpdated(uint256 feeBps);

    // ─── Constants ────────────────────────────────────────────────────────

    uint256 public constant MAX_PERFORMANCE_FEE = 2000; // 20%
    uint256 public constant MAX_WITHDRAWAL_FEE  = 100;  // 1%

    // ─── State ────────────────────────────────────────────────────────────

    address public owner;
    address public treasury;

    /// @notice The ERC-20 asset deposited by users (e.g. LP token, ZBX).
    address public asset;

    /// @notice External farm where asset is deployed.
    address public farm;

    /// @notice Reward token paid by the farm.
    address public rewardToken;

    /// @notice Swap router to convert rewards → asset.
    address public router;

    /// @notice Swap path: [rewardToken, ..., asset].
    address[] public swapPath;

    mapping(address => bool) public isKeeper;

    // ─── Share accounting ─────────────────────────────────────────────────

    uint256 public totalShares;
    mapping(address => uint256) public shares;

    // ─── Fee config ───────────────────────────────────────────────────────

    uint256 public performanceFeeBps = 1000; // 10%
    uint256 public withdrawalFeeBps  = 10;   // 0.1%

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor(
        address asset_,
        address farm_,
        address rewardToken_,
        address router_,
        address[] memory swapPath_,
        address treasury_
    ) {
        require(asset_ != address(0) && farm_ != address(0) && treasury_ != address(0),
                "Optimizer: zero address");
        owner       = msg.sender;
        asset       = asset_;
        farm        = farm_;
        rewardToken = rewardToken_;
        router      = router_;
        swapPath    = swapPath_;
        treasury    = treasury_;
        isKeeper[msg.sender] = true;
    }

    // ─── Deposit ──────────────────────────────────────────────────────────

    /// @notice Deposit `amount` of asset.  Receives vault shares.
    function deposit(uint256 amount) external nonReentrant returns (uint256 sharesOut) {
        if (amount == 0) revert ZeroAmount();

        uint256 poolBefore = _totalAssets();
        require(IERC20Opt(asset).transferFrom(msg.sender, address(this), amount),
                "Optimizer: transfer failed");

        if (totalShares == 0 || poolBefore == 0) {
            sharesOut = amount;
        } else {
            sharesOut = (amount * totalShares) / poolBefore;
        }

        shares[msg.sender] += sharesOut;
        totalShares         += sharesOut;

        // Deploy asset into farm
        IERC20Opt(asset).approve(farm, amount);
        IFarm(farm).deposit(amount);

        emit Deposited(msg.sender, amount, sharesOut);
    }

    // ─── Withdraw ─────────────────────────────────────────────────────────

    /// @notice Burn `sharesIn` and receive proportional asset (minus withdrawal fee).
    function withdraw(uint256 sharesIn) external nonReentrant returns (uint256 assetsOut) {
        if (sharesIn == 0)                 revert ZeroAmount();
        if (shares[msg.sender] < sharesIn) revert InsufficientShares();

        uint256 totalAssets = _totalAssets();
        assetsOut = (sharesIn * totalAssets) / totalShares;

        shares[msg.sender] -= sharesIn;
        totalShares         -= sharesIn;

        // Withdraw from farm
        IFarm(farm).withdraw(assetsOut);

        uint256 fee = (assetsOut * withdrawalFeeBps) / 10_000;
        uint256 out = assetsOut - fee;

        if (fee > 0) IERC20Opt(asset).transfer(treasury, fee);
        IERC20Opt(asset).transfer(msg.sender, out);

        emit Withdrawn(msg.sender, sharesIn, out);
    }

    // ─── Compound ─────────────────────────────────────────────────────────

    /// @notice Claim farm rewards, swap to asset, and re-deposit.
    ///         Anyone whitelisted as keeper can call this.
    // SOL-05 (MEDIUM): nonReentrant added — compound makes external calls to
    // untrusted farm (claim) and router (swap). A malicious farm or router
    // could re-enter deposit/withdraw and corrupt share accounting.
    function compound(uint256 minAssetOut) external nonReentrant {
        if (!isKeeper[msg.sender]) revert NotKeeper();

        // Claim rewards from farm
        IFarm(farm).claim();
        uint256 rewardBal = IERC20Opt(rewardToken).balanceOf(address(this));
        if (rewardBal == 0) return;

        // Swap rewards → asset via router
        IERC20Opt(rewardToken).approve(router, rewardBal);
        uint256[] memory amounts = ISwapRouter(router).swapExactTokensForTokens(
            rewardBal, minAssetOut, swapPath, address(this), block.timestamp + 300
        );
        uint256 assetGained = amounts[amounts.length - 1];

        // Performance fee
        uint256 fee = (assetGained * performanceFeeBps) / 10_000;
        if (fee > 0) IERC20Opt(asset).transfer(treasury, fee);
        uint256 toReinvest = assetGained - fee;

        // Re-deposit into farm
        if (toReinvest > 0) {
            IERC20Opt(asset).approve(farm, toReinvest);
            IFarm(farm).deposit(toReinvest);
        }

        emit Compounded(rewardBal, toReinvest, fee);
    }

    // ─── View helpers ─────────────────────────────────────────────────────

    /// @notice Current asset value of `sharesAmount`.
    function previewWithdraw(uint256 sharesAmount) external view returns (uint256) {
        if (totalShares == 0) return 0;
        return (sharesAmount * _totalAssets()) / totalShares;
    }

    /// @notice Current exchange rate: assets per share (18-decimal).
    function pricePerShare() external view returns (uint256) {
        if (totalShares == 0) return 1e18;
        return (_totalAssets() * 1e18) / totalShares;
    }

    /// @notice Pending rewards not yet compounded.
    function pendingRewards() external view returns (uint256) {
        return IFarm(farm).pendingReward(address(this));
    }

    function _totalAssets() internal view returns (uint256) {
        return IERC20Opt(asset).balanceOf(address(this))
             + IERC20Opt(asset).balanceOf(farm); // simplification; real: query farm balance
    }

    // ─── Admin ────────────────────────────────────────────────────────────

    function setKeeper(address k, bool enabled) external {
        if (msg.sender != owner) revert NotOwner();
        isKeeper[k] = enabled;
        emit KeeperSet(k, enabled);
    }

    function setPerformanceFee(uint256 bps) external {
        if (msg.sender != owner) revert NotOwner();
        if (bps > MAX_PERFORMANCE_FEE) revert FeeTooHigh();
        performanceFeeBps = bps;
        emit PerformanceFeeUpdated(bps);
    }

    function setWithdrawalFee(uint256 bps) external {
        if (msg.sender != owner) revert NotOwner();
        if (bps > MAX_WITHDRAWAL_FEE) revert FeeTooHigh();
        withdrawalFeeBps = bps;
        emit WithdrawalFeeUpdated(bps);
    }
}
