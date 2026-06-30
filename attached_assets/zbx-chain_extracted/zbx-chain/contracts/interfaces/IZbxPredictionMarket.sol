// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxPredictionMarket — Interface for ZbxPredictionMarket binary outcome prediction market.
interface IZbxPredictionMarket {
    enum Outcome { Unresolved, Yes, No, Cancelled }

    struct Market {
        string  question;
        uint256 closesAt;
        uint256 resolvedAt;
        Outcome outcome;
        uint256 yesPool;
        uint256 noPool;
        address resolver;
    }

    event MarketCreated(uint256 indexed marketId, string question, uint256 closesAt);
    event Bet(uint256 indexed marketId, address indexed bettor, bool isYes, uint256 amount);
    event Resolved(uint256 indexed marketId, Outcome outcome);
    event Claimed(uint256 indexed marketId, address indexed bettor, uint256 amount);
    event Refunded(uint256 indexed marketId, address indexed bettor, uint256 amount);

    error MarketNotFound();
    error MarketNotOpen();
    error MarketNotResolved();
    error NothingToClaim();

    function createMarket(string calldata question, uint256 closesAt, address resolver) external returns (uint256 marketId);
    function bet(uint256 marketId, bool isYes) external payable;
    function resolve(uint256 marketId, Outcome outcome) external;
    function claim(uint256 marketId) external;
    function refund(uint256 marketId) external;
    function getMarket(uint256 marketId) external view returns (Market memory);
    function getBet(uint256 marketId, address bettor) external view returns (uint256 yesAmount, uint256 noAmount);
}
