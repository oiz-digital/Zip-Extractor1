// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxSpotOrderBook — Interface for ZbxSpotOrderBook on-chain spot trading order book.
interface IZbxSpotOrderBook {
    enum OrderSide { Buy, Sell }
    enum OrderStatus { Open, Filled, PartialFill, Cancelled }

    struct Order {
        bytes32   orderId;
        address   trader;
        OrderSide side;
        address   baseToken;
        address   quoteToken;
        uint256   price;
        uint256   amount;
        uint256   filled;
        OrderStatus status;
        uint256   createdAt;
    }

    event OrderPlaced(bytes32 indexed orderId, address indexed trader, OrderSide side, uint256 price, uint256 amount);
    event OrderMatched(bytes32 indexed buyOrderId, bytes32 indexed sellOrderId, uint256 price, uint256 amount);
    event OrderCancelled(bytes32 indexed orderId);

    function placeOrder(address baseToken, address quoteToken, OrderSide side, uint256 price, uint256 amount) external returns (bytes32 orderId);
    function cancelOrder(bytes32 orderId) external;
    function matchOrders(bytes32 buyOrderId, bytes32 sellOrderId) external;
    function getOrder(bytes32 orderId) external view returns (Order memory);
    function getOpenOrders(address trader) external view returns (bytes32[] memory);
    function getBestBid(address baseToken, address quoteToken) external view returns (uint256);
    function getBestAsk(address baseToken, address quoteToken) external view returns (uint256);
}
