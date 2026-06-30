// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxCardGame — Interface for ZbxCardGame on-chain trading card game.
interface IZbxCardGame {
    struct Card {
        uint256 cardId;
        uint8   rarity;
        uint8   attack;
        uint8   defense;
        string  uri;
    }

    event CardMinted(address indexed to, uint256 indexed cardId, uint8 rarity);
    event PackOpened(address indexed opener, uint256[] cardIds);
    event CardTraded(address indexed from, address indexed to, uint256 indexed cardId);
    event DuelResult(address indexed winner, address indexed loser, uint256 wagerAmount);

    function openPack() external payable returns (uint256[] memory cardIds);
    function trade(address to, uint256 cardId) external;
    function duel(address opponent, uint256[] calldata myCards, uint256 wager) external payable;
    function getCard(uint256 cardId) external view returns (Card memory);
    function cardsOf(address player) external view returns (uint256[] memory);
    function packPrice() external view returns (uint256);
}
