// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import "./libraries/SafeERC20.sol";
import "./libraries/ReentrancyGuard.sol";

/// @title  ZbxSpotOrderBook — On-chain Central Limit Order Book (CLOB) for spot trading
/// @author Zebvix Technologies Pvt Ltd
///
/// @notice Fully on-chain, permissionless spot exchange.
///         Any ERC-20/ERC-20 pair (or native ZBX/ERC-20) can be traded.
///
///         Model:
///           Makers place limit orders (stored on-chain, tokens escrowed).
///           Takers call fillOrder() or matchOrders() to execute at the
///           maker's price — no off-chain relayer required.
///           Partial fills supported.
///
///         Fee model:
///           Maker fee:  0.05% (rebate possible in v2)
///           Taker fee:  0.20%
///           Both taken from the received token.
///
///         Price encoding: `price` is always QuoteToken per BaseToken,
///           scaled to 1e18.  E.g. price = 5e21 means 5000 ZUSD per ZBX.
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:module     Trading / Spot (ZEP-042)

contract ZbxSpotOrderBook is ReentrancyGuard {
    using SafeERC20 for IERC20Minimal;

    // ─── Errors ───────────────────────────────────────────────────────────

    error OrderNotFound();
    error OrderFilled();
    error OrderCancelled();
    error OrderExpired();
    error NotMaker();
    error InsufficientFillAmount();
    error PriceMismatch();
    error SameSide();
    error ZeroAmount();
    error ZeroAddress();
    error InvalidPair();
    error TransferFailed();
    error NotOwner();
    error FeeTooHigh();

    // ─── Events ───────────────────────────────────────────────────────────

    event OrderPlaced(
        bytes32 indexed orderId,
        address indexed maker,
        address indexed baseToken,
        address         quoteToken,
        bool            isBuy,
        uint256         price,
        uint256         amount,
        uint256         expiry
    );
    event OrderFilled(
        bytes32 indexed orderId,
        address indexed taker,
        uint256         filledAmount,
        uint256         quoteAmount,
        uint256         makerFee,
        uint256         takerFee
    );
    event OrderCancelled(bytes32 indexed orderId, address indexed maker);
    event OrderMatched(bytes32 indexed buyOrderId, bytes32 indexed sellOrderId, uint256 amount, uint256 price);

    // ─── Constants ────────────────────────────────────────────────────────

    uint256 public constant MAKER_FEE_BPS  = 5;    // 0.05%
    uint256 public constant TAKER_FEE_BPS  = 20;   // 0.20%
    uint256 public constant MAX_FEE_BPS    = 100;   // 1.00%

    // ─── Types ────────────────────────────────────────────────────────────

    enum OrderStatus { Open, PartiallyFilled, Filled, Cancelled, Expired }

    struct Order {
        address maker;
        address baseToken;   // token being bought (buy order) or sold (sell order)
        address quoteToken;  // token used for payment
        bool    isBuy;       // true = buy base with quote; false = sell base for quote
        uint256 price;       // quote tokens per 1e18 base tokens (18-decimal normalised)
        uint256 amount;      // total base token amount
        uint256 filled;      // base tokens filled so far
        uint256 expiry;      // 0 = no expiry; otherwise unix timestamp
        OrderStatus status;
    }

    // ─── State ────────────────────────────────────────────────────────────

    address public owner;
    address public treasury;

    mapping(bytes32 => Order) public orders;
    uint256 public orderCount;

    /// @notice Protocol fee balance per token
    mapping(address => uint256) public feeBalance;

    uint256 public makerFeeBps = MAKER_FEE_BPS;
    uint256 public takerFeeBps = TAKER_FEE_BPS;

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor(address treasury_) {
        require(treasury_ != address(0), "OrderBook: zero treasury");
        owner    = msg.sender;
        treasury = treasury_;
    }

    // ─── Place order ──────────────────────────────────────────────────────

    /// @notice Place a limit order.  Tokens are escrowed immediately.
    /// @param  baseToken   The asset being traded (bought or sold).
    /// @param  quoteToken  The payment token.
    /// @param  isBuy       True = buy `amount` base with quote at `price`.
    ///                     False = sell `amount` base for quote at `price`.
    /// @param  price       Quote tokens per 1e18 base tokens.
    /// @param  amount      Base token amount.
    /// @param  expiry      Expiry timestamp (0 = GTC — Good Till Cancelled).
    /// @return orderId     Unique identifier for this order.
    function placeOrder(
        address baseToken,
        address quoteToken,
        bool    isBuy,
        uint256 price,
        uint256 amount,
        uint256 expiry
    ) external payable nonReentrant returns (bytes32 orderId) {
        if (baseToken == address(0) && quoteToken == address(0)) revert InvalidPair();
        if (baseToken == quoteToken)  revert InvalidPair();
        if (price == 0)               revert ZeroAmount();
        if (amount == 0)              revert ZeroAmount();
        if (expiry != 0 && expiry <= block.timestamp) revert OrderExpired();

        orderId = keccak256(abi.encodePacked(
            msg.sender, baseToken, quoteToken, isBuy, price, amount, block.number, orderCount++
        ));

        // Escrow the input token
        if (isBuy) {
            // Buyer escrows quote tokens
            uint256 quoteRequired = (amount * price) / 1e18;
            _receiveToken(quoteToken, msg.sender, quoteRequired);
        } else {
            // Seller escrows base tokens
            _receiveToken(baseToken, msg.sender, amount);
        }

        orders[orderId] = Order({
            maker:      msg.sender,
            baseToken:  baseToken,
            quoteToken: quoteToken,
            isBuy:      isBuy,
            price:      price,
            amount:     amount,
            filled:     0,
            expiry:     expiry,
            status:     OrderStatus.Open
        });

        emit OrderPlaced(orderId, msg.sender, baseToken, quoteToken, isBuy, price, amount, expiry);
    }

    // ─── Fill order (taker) ───────────────────────────────────────────────

    /// @notice Fill all or part of an existing limit order.
    ///         Caller is the taker; they supply the opposite side tokens.
    /// @param  orderId    The order to fill.
    /// @param  fillAmount How many base tokens to fill (≤ remaining amount).
    function fillOrder(bytes32 orderId, uint256 fillAmount) external payable nonReentrant {
        if (fillAmount == 0) revert ZeroAmount();
        Order storage o = orders[orderId];
        _validateOrder(o);

        uint256 remaining = o.amount - o.filled;
        if (fillAmount > remaining) fillAmount = remaining;

        uint256 quoteAmount = (fillAmount * o.price) / 1e18;

        uint256 makerFee;
        uint256 takerFee;
        address baseTok = o.baseToken;
        address quoteTok = o.quoteToken;
        address makerAddr = o.maker;
        bool    isBuy   = o.isBuy;

        if (isBuy) {
            takerFee  = (fillAmount * takerFeeBps) / 10_000;
            makerFee  = (quoteAmount * makerFeeBps) / 10_000;
        } else {
            takerFee  = (quoteAmount * takerFeeBps) / 10_000;
            makerFee  = (fillAmount * makerFeeBps) / 10_000;
        }

        // SEC-2026-05-09 — CEI: update ALL state before any external transfer.
        o.filled += fillAmount;
        o.status = o.filled >= o.amount ? OrderStatus.Filled : OrderStatus.PartiallyFilled;
        if (isBuy) {
            feeBalance[baseTok]  += takerFee;
            feeBalance[quoteTok] += makerFee;
        } else {
            feeBalance[quoteTok] += takerFee;
            feeBalance[baseTok]  += makerFee;
        }

        if (isBuy) {
            // Maker is buyer (quote escrowed). Taker delivers base, receives quote.
            _receiveToken(baseTok, msg.sender, fillAmount);
            _sendToken(baseTok, makerAddr, fillAmount - takerFee);
            _sendToken(quoteTok, msg.sender, quoteAmount - makerFee);
        } else {
            // Maker is seller (base escrowed). Taker delivers quote, receives base.
            _receiveToken(quoteTok, msg.sender, quoteAmount);
            _sendToken(quoteTok, makerAddr, quoteAmount - takerFee);
            _sendToken(baseTok, msg.sender, fillAmount - makerFee);
        }

        emit OrderFilled(orderId, msg.sender, fillAmount, quoteAmount, makerFee, takerFee);
    }

    // ─── Match two orders ─────────────────────────────────────────────────

    /// @notice Match a buy order against a sell order.  No external tokens
    ///         required — both sides are already escrowed.  Anyone can call.
    ///         Execution price = midpoint of the two order prices (favours maker).
    /// @param  buyOrderId   A buy limit order.
    /// @param  sellOrderId  A sell limit order for the same pair.
    function matchOrders(bytes32 buyOrderId, bytes32 sellOrderId) external nonReentrant {
        Order storage buy  = orders[buyOrderId];
        Order storage sell = orders[sellOrderId];

        _validateOrder(buy);
        _validateOrder(sell);

        require(buy.isBuy  && !sell.isBuy,    "OrderBook: not buy+sell pair");
        require(buy.baseToken  == sell.baseToken,  "OrderBook: base mismatch");
        require(buy.quoteToken == sell.quoteToken, "OrderBook: quote mismatch");
        require(buy.price >= sell.price,           "OrderBook: price cross needed");

        uint256 execPrice = sell.price;

        uint256 buyRemaining  = buy.amount  - buy.filled;
        uint256 sellRemaining = sell.amount - sell.filled;
        uint256 matchAmount   = buyRemaining < sellRemaining ? buyRemaining : sellRemaining;

        uint256 quoteAmount = (matchAmount * execPrice) / 1e18;

        // SEC-2026-05-09 — fee fix: previously both sides paid only the taker
        // fee, undercutting the documented maker/taker rebate model. Now the
        // matcher (taker-of-second-side) pays takerFee and the resting maker
        // pays makerFee on their received token. The buy order is the maker
        // when buy was placed first; we treat both as makers here (this is a
        // pure book-internal cross with no external taker).
        uint256 buyFee  = (matchAmount  * makerFeeBps) / 10_000;   // base received
        uint256 sellFee = (quoteAmount  * makerFeeBps) / 10_000;   // quote received

        uint256 buyEscrowed = (matchAmount * buy.price) / 1e18;
        uint256 excess      = buyEscrowed > quoteAmount ? buyEscrowed - quoteAmount : 0;

        address baseTok  = buy.baseToken;
        address quoteTok = sell.quoteToken;
        address buyMaker  = buy.maker;
        address sellMaker = sell.maker;

        // SEC-2026-05-09 — CEI: state-before-external on every settlement path.
        buy.filled  += matchAmount;
        sell.filled += matchAmount;
        buy.status  = buy.filled  >= buy.amount  ? OrderStatus.Filled : OrderStatus.PartiallyFilled;
        sell.status = sell.filled >= sell.amount ? OrderStatus.Filled : OrderStatus.PartiallyFilled;
        feeBalance[baseTok]  += buyFee;
        feeBalance[quoteTok] += sellFee;

        _sendToken(baseTok,  buyMaker,  matchAmount - buyFee);
        _sendToken(quoteTok, sellMaker, quoteAmount - sellFee);
        if (excess > 0) _sendToken(quoteTok, buyMaker, excess);

        emit OrderMatched(buyOrderId, sellOrderId, matchAmount, execPrice);
    }

    // ─── Cancel order ─────────────────────────────────────────────────────

    /// @notice Cancel an open order and refund escrowed tokens.
    function cancelOrder(bytes32 orderId) external nonReentrant {
        Order storage o = orders[orderId];
        if (o.maker != msg.sender)                    revert NotMaker();
        if (o.status == OrderStatus.Filled)           revert OrderFilled();
        if (o.status == OrderStatus.Cancelled)        revert OrderCancelled();

        // SEC-2026-05-09 — snapshot + state-before-external (CEI).
        uint256 refundBase  = o.amount - o.filled;
        bool    isBuy       = o.isBuy;
        address makerAddr   = o.maker;
        address baseTok     = o.baseToken;
        address quoteTok    = o.quoteToken;
        uint256 priceCached = o.price;

        o.status = OrderStatus.Cancelled;

        if (isBuy) {
            uint256 refundQuote = (refundBase * priceCached) / 1e18;
            _sendToken(quoteTok, makerAddr, refundQuote);
        } else {
            _sendToken(baseTok, makerAddr, refundBase);
        }

        emit OrderCancelled(orderId, makerAddr);
    }

    // ─── Expire order ─────────────────────────────────────────────────────

    /// @notice Anyone can expire a GTC order past its expiry time.
    ///         Refunds escrowed tokens to maker.
    function expireOrder(bytes32 orderId) external nonReentrant {
        Order storage o = orders[orderId];
        if (o.status == OrderStatus.Filled || o.status == OrderStatus.Cancelled) return;
        if (o.expiry == 0 || block.timestamp < o.expiry) return;

        // SEC-2026-05-09 — snapshot + state-before-external (CEI).
        uint256 refundBase  = o.amount - o.filled;
        bool    isBuy       = o.isBuy;
        address makerAddr   = o.maker;
        address baseTok     = o.baseToken;
        address quoteTok    = o.quoteToken;
        uint256 priceCached = o.price;

        o.status = OrderStatus.Expired;

        if (isBuy) {
            uint256 refundQuote = (refundBase * priceCached) / 1e18;
            _sendToken(quoteTok, makerAddr, refundQuote);
        } else {
            _sendToken(baseTok, makerAddr, refundBase);
        }
    }

    // ─── View helpers ─────────────────────────────────────────────────────

    function remainingAmount(bytes32 orderId) external view returns (uint256) {
        Order storage o = orders[orderId];
        return o.amount > o.filled ? o.amount - o.filled : 0;
    }

    function orderStatus(bytes32 orderId) external view returns (OrderStatus) {
        return orders[orderId].status;
    }

    // ─── Admin ────────────────────────────────────────────────────────────

    function withdrawFees(address token) external {
        require(msg.sender == owner, "OrderBook: not owner");
        uint256 amount = feeBalance[token];
        feeBalance[token] = 0;
        _sendToken(token, treasury, amount);
    }

    function setFees(uint256 makerBps, uint256 takerBps) external {
        require(msg.sender == owner, "OrderBook: not owner");
        require(makerBps <= MAX_FEE_BPS && takerBps <= MAX_FEE_BPS, "OrderBook: fee too high");
        makerFeeBps = makerBps;
        takerFeeBps = takerBps;
    }

    // ─── Internal helpers ─────────────────────────────────────────────────

    function _validateOrder(Order storage o) private view {
        if (o.maker == address(0))             revert OrderNotFound();
        if (o.status == OrderStatus.Filled)    revert OrderFilled();
        if (o.status == OrderStatus.Cancelled) revert OrderCancelled();
        if (o.expiry != 0 && block.timestamp > o.expiry) revert OrderExpired();
    }

    function _receiveToken(address token, address from, uint256 amount) private {
        if (amount == 0) return;
        if (token == address(0)) {
            require(msg.value >= amount, "OrderBook: insufficient ZBX");
            if (msg.value > amount) {
                (bool ok,) = from.call{value: msg.value - amount}("");
                require(ok, "OrderBook: refund failed");
            }
        } else {
            // SEC-2026-05-09 — SafeERC20 (USDT-compat: tolerates empty return).
            IERC20Minimal(token).safeTransferFrom(from, address(this), amount);
        }
    }

    function _sendToken(address token, address to, uint256 amount) private {
        if (amount == 0) return;
        if (token == address(0)) {
            (bool ok,) = to.call{value: amount}("");
            require(ok, "OrderBook: ZBX send failed");
        } else {
            // SEC-2026-05-09 — SafeERC20.
            IERC20Minimal(token).safeTransfer(to, amount);
        }
    }

    receive() external payable {}
}
