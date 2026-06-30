// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZusdVault — Interface for the ZUSD CDP vault.
interface IZusdVault {
    struct CDP {
        uint256 collateral;
        uint256 debt;
        uint256 lastFeeIndex;
        uint256 openedAt;
    }

    event CDPOpened(address indexed owner, uint256 collateral, uint256 debt);
    event CollateralAdded(address indexed owner, uint256 amount);
    event ZusdMinted(address indexed owner, uint256 amount);
    event ZusdRepaid(address indexed owner, uint256 amount);
    event CollateralWithdrawn(address indexed owner, uint256 amount);
    event CDPLiquidated(
        address indexed owner, address indexed liquidator,
        uint256 debtRepaid, uint256 collateralSeized, uint256 bonus
    );
    event Redeemed(address indexed redeemer, uint256 zusdAmount, uint256 zbxReceived);
    event RedeemedFromCDP(address indexed cdpOwner, uint256 zusdRedeemed, uint256 zbxTaken);
    event CDPClosedByRedemption(address indexed cdpOwner, uint256 leftoverCollateralReturned);
    event RedemptionPausedToggled(address indexed by, bool paused);
    event FeeRecipientUpdated(address indexed newRecipient);

    function openCDP(uint256 collateralAmount, uint256 zusdAmount) external;
    function addCollateral(uint256 amount) external;
    function mintMore(uint256 zusdAmount) external;
    function repay(uint256 zusdAmount) external;
    function withdrawCollateral(uint256 amount) external;
    function closeCDP() external;
    function liquidate(address cdpOwner) external;

    /// @notice Redeem ZUSD for ZBX from CDPs in ascending-CR order.
    /// @param  zusdAmount      Total ZUSD redeemer wants to redeem.
    /// @param  cdpHints        Off-chain-sorted CDP owners (ascending CR).
    /// @param  maxIterations   Max CDPs to traverse (gas bound, <= 50).
    /// @return zusdRedeemed    Actual ZUSD burned (may be < zusdAmount if hints exhausted).
    /// @return zbxOut          ZBX sent to redeemer (post-fee).
    function redeem(
        uint256 zusdAmount,
        address[] calldata cdpHints,
        uint256 maxIterations
    ) external returns (uint256 zusdRedeemed, uint256 zbxOut);

    /// @notice Owner-only emergency pause for redemptions.
    function setRedemptionPaused(bool paused) external;

    /// @notice Owner-only setter for redemption-fee recipient.
    function setFeeRecipient(address recipient) external;

    function getCDP(address user) external view returns (
        uint256 collateral, uint256 debt, uint256 collateralRatioBps, bool liquidatable
    );
    function maxMintable(uint256 zbxAmount) external view returns (uint256);

    function totalCollateral()       external view returns (uint256);
    function totalDebt()             external view returns (uint256);
    function feeIndex()              external view returns (uint256);
    function redemptionPaused()      external view returns (bool);
    function feeRecipient()          external view returns (address);
    function totalRedeemed()         external view returns (uint256);
    function totalRedemptionFees()   external view returns (uint256);
}