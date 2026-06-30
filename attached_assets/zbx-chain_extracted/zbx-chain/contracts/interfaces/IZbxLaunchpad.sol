// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxLaunchpad — Interface for ZbxLaunchpad IDO/token sale contract.
interface IZbxLaunchpad {
    struct Sale {
        address token;
        uint256 price;
        uint256 hardCap;
        uint256 softCap;
        uint256 raised;
        uint256 startTime;
        uint256 endTime;
        uint256 cliffBlocks;
        bool    finalized;
        bool    cancelled;
    }

    event SaleCreated(uint256 indexed saleId, address indexed token, uint256 price, uint256 hardCap);
    event Purchased(uint256 indexed saleId, address indexed buyer, uint256 amount, uint256 cost);
    event Claimed(uint256 indexed saleId, address indexed buyer, uint256 amount);
    event Refunded(uint256 indexed saleId, address indexed buyer, uint256 amount);

    error SaleNotFound();
    error SaleNotOpen();
    error CapReached();
    error AlreadyClaimed();

    function createSale(address token, uint256 price, uint256 hardCap, uint256 softCap, uint256 startTime, uint256 endTime, uint256 cliffBlocks, address[] calldata whitelist) external returns (uint256 saleId);
    function buy(uint256 saleId, uint256 amount) external payable;
    function claim(uint256 saleId) external;
    function refund(uint256 saleId) external;
    function finalize(uint256 saleId) external;
    function cancel(uint256 saleId) external;
    function getSale(uint256 saleId) external view returns (Sale memory);
    function getPurchase(uint256 saleId, address buyer) external view returns (uint256 amount, bool claimed);
}
