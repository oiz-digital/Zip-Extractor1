// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxRaffle — Interface for ZbxRaffle on-chain provably-fair raffle.
interface IZbxRaffle {
    enum Status { Open, Drawing, Settled, Cancelled }

    struct Raffle {
        address  creator;
        address  prizeToken;
        uint256  prizeAmount;
        uint256  ticketPrice;
        uint256  maxTickets;
        uint256  sold;
        uint256  closesAt;
        Status   status;
        address  winner;
    }

    event RaffleCreated(uint256 indexed raffleId, address creator, uint256 ticketPrice, uint256 maxTickets, uint256 closesAt);
    event TicketBought(uint256 indexed raffleId, address indexed buyer, uint256 quantity);
    event WinnerDrawn(uint256 indexed raffleId, address indexed winner, uint256 prize);
    event Refunded(uint256 indexed raffleId, address indexed buyer, uint256 amount);

    function createRaffle(address prizeToken, uint256 prizeAmount, uint256 ticketPrice, uint256 maxTickets, uint256 duration) external payable returns (uint256 raffleId);
    function buyTickets(uint256 raffleId, uint256 quantity) external payable;
    function draw(uint256 raffleId) external;
    function claimPrize(uint256 raffleId) external;
    function refund(uint256 raffleId) external;
    function getRaffle(uint256 raffleId) external view returns (Raffle memory);
    function getTicketCount(uint256 raffleId, address buyer) external view returns (uint256);
}
