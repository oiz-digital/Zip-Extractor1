// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title  ZbxPaymentGateway — Crypto payment processor for Zebvix Chain
/// @author Zebvix Technologies Pvt Ltd
///
/// @notice Full-featured crypto payment gateway:
///           - Merchants register and receive a unique merchantId.
///           - Merchants create invoices (payment links) specifying token,
///             amount, expiry, and an off-chain orderId.
///           - Customers pay invoices in the required token, or in any
///             other token via auto-swap through ZbxRouter.
///           - Partial payments are accumulated until the invoice is fully
///             paid (useful for subscriptions / variable pricing).
///           - Merchants withdraw accumulated balances any time.
///           - Refunds: merchant can refund a paid invoice within a
///             REFUND_WINDOW; customer receives the exact input amount.
///           - Protocol fee (default 0.5 %) goes to the gateway treasury.
///           - Events are structured for off-chain webhook / subgraph indexing.
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:module     Payment Gateway (ZEP-032)

interface IERC20Pay {
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function transfer(address to, uint256 amount) external returns (bool);
    function approve(address spender, uint256 amount) external returns (bool);
    function balanceOf(address account) external view returns (uint256);
}

interface IZbxRouterPay {
    function swapTokensForExactTokens(
        uint256 amountOut,
        uint256 amountInMax,
        address[] calldata path,
        address to,
        uint256 deadline
    ) external returns (uint256[] memory amounts);
}

import { ReentrancyGuard } from "./libraries/ReentrancyGuard.sol";

contract ZbxPaymentGateway is ReentrancyGuard {

    // ─── Errors ───────────────────────────────────────────────────────────

    error NotMerchant();
    error InvoiceNotFound();
    error InvoiceExpired();
    error InvoiceAlreadyPaid();
    error InvoiceCancelled();
    error RefundWindowPassed();
    error NotPaid();
    error ZeroAddress();
    error InvalidAmount();
    error InvalidExpiry();
    error InvalidFeeBps();
    error InsufficientOutput();
    error WithdrawFailed();
    error Unauthorised();

    // ─── Events (structured for off-chain webhook indexing) ───────────────

    event MerchantRegistered(
        bytes32 indexed merchantId,
        address indexed owner,
        string  name,
        address payoutAddress
    );
    event MerchantUpdated(bytes32 indexed merchantId, address payoutAddress);

    event InvoiceCreated(
        bytes32 indexed invoiceId,
        bytes32 indexed merchantId,
        bytes32 indexed orderId,        // off-chain order reference
        address         token,          // required payment token
        uint256         amount,         // required amount (18-decimal)
        uint256         expiresAt       // unix timestamp
    );
    event InvoicePaid(
        bytes32 indexed invoiceId,
        bytes32 indexed merchantId,
        address indexed payer,
        address         inputToken,     // token the customer actually sent
        uint256         inputAmount,    // amount customer actually sent
        uint256         amountPaid,     // invoice token amount received
        uint256         fee
    );
    event InvoiceCancelledEv(bytes32 indexed invoiceId, bytes32 indexed merchantId);

    event Refunded(
        bytes32 indexed invoiceId,
        address indexed payer,
        uint256         amount
    );
    event MerchantWithdrawal(
        bytes32 indexed merchantId,
        address         token,
        uint256         amount
    );
    event ProtocolFeeUpdated(uint256 newFeeBps);
    event TreasuryUpdated(address newTreasury);

    // ─── Types ────────────────────────────────────────────────────────────

    struct Merchant {
        address owner;
        string  name;
        address payoutAddress;
        bool    active;
    }

    enum InvoiceStatus { Pending, Paid, Cancelled }

    struct Invoice {
        bytes32       merchantId;
        bytes32       orderId;
        address       token;
        uint256       amount;          // total amount required
        uint256       amountPaid;      // accumulated so far
        uint256       expiresAt;
        InvoiceStatus status;
        address       lastPayer;
        uint256       lastInputAmount; // for refund tracking
        address       lastInputToken;
    }

    // ─── Constants ────────────────────────────────────────────────────────

    /// @notice Maximum refund window after payment (48 hours).
    uint256 public constant REFUND_WINDOW = 48 hours;

    /// @notice Maximum protocol fee (2 %).
    uint256 public constant MAX_FEE_BPS = 200;

    // ─── State ────────────────────────────────────────────────────────────

    address public gatewayOwner;
    address public treasury;

    /// @notice Protocol fee in basis points (default 50 = 0.5 %).
    uint256 public feeBps = 50;

    mapping(bytes32 => Merchant) public merchants;
    mapping(bytes32 => Invoice)  public invoices;

    /// @notice merchantId → token → accrued balance ready to withdraw
    mapping(bytes32 => mapping(address => uint256)) public merchantBalances;

    /// @notice invoiceId → payment timestamp (for refund window check)
    mapping(bytes32 => uint256) public paidAt;

    uint256 private _nonce;

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor(address treasury_) {
        require(treasury_ != address(0), "Gateway: zero treasury");
        gatewayOwner = msg.sender;
        treasury     = treasury_;
    }

    // ─── Merchant registration ────────────────────────────────────────────

    /// @notice Register as a merchant.  Returns a unique merchantId.
    /// @param  name           Business name (for events / explorer).
    /// @param  payoutAddress  Address to receive withdrawals.
    function registerMerchant(string calldata name, address payoutAddress)
        external
        returns (bytes32 merchantId)
    {
        if (payoutAddress == address(0)) revert ZeroAddress();

        merchantId = keccak256(abi.encodePacked(
            msg.sender, name, block.timestamp, ++_nonce
        ));

        merchants[merchantId] = Merchant({
            owner:         msg.sender,
            name:          name,
            payoutAddress: payoutAddress,
            active:        true
        });

        emit MerchantRegistered(merchantId, msg.sender, name, payoutAddress);
    }

    /// @notice Update payout address.
    function updatePayoutAddress(bytes32 merchantId, address newPayout) external {
        Merchant storage m = merchants[merchantId];
        if (m.owner != msg.sender) revert NotMerchant();
        if (newPayout == address(0)) revert ZeroAddress();
        m.payoutAddress = newPayout;
        emit MerchantUpdated(merchantId, newPayout);
    }

    // ─── Invoice management ───────────────────────────────────────────────

    /// @notice Create a payment invoice (payment link).
    /// @param  merchantId  Your merchantId from registerMerchant().
    /// @param  token       ERC-20 token the customer must pay in.
    /// @param  amount      Exact amount required (in token's decimals).
    /// @param  expiresAt   Unix timestamp when the invoice expires.
    /// @param  orderId     Off-chain order reference (for webhook matching).
    /// @return invoiceId   Unique invoice identifier.
    function createInvoice(
        bytes32 merchantId,
        address token,
        uint256 amount,
        uint256 expiresAt,
        bytes32 orderId
    ) external returns (bytes32 invoiceId) {
        if (merchants[merchantId].owner != msg.sender) revert NotMerchant();
        if (token == address(0))   revert ZeroAddress();
        if (amount == 0)           revert InvalidAmount();
        if (expiresAt <= block.timestamp) revert InvalidExpiry();

        invoiceId = keccak256(abi.encodePacked(
            merchantId, orderId, token, amount, expiresAt, ++_nonce
        ));

        invoices[invoiceId] = Invoice({
            merchantId:      merchantId,
            orderId:         orderId,
            token:           token,
            amount:          amount,
            amountPaid:      0,
            expiresAt:       expiresAt,
            status:          InvoiceStatus.Pending,
            lastPayer:       address(0),
            lastInputAmount: 0,
            lastInputToken:  address(0)
        });

        emit InvoiceCreated(invoiceId, merchantId, orderId, token, amount, expiresAt);
    }

    /// @notice Cancel an unpaid invoice.
    function cancelInvoice(bytes32 invoiceId) external {
        Invoice storage inv = invoices[invoiceId];
        if (inv.amount == 0) revert InvoiceNotFound();
        if (merchants[inv.merchantId].owner != msg.sender) revert NotMerchant();
        if (inv.status != InvoiceStatus.Pending) revert InvoiceAlreadyPaid();
        inv.status = InvoiceStatus.Cancelled;
        emit InvoiceCancelledEv(invoiceId, inv.merchantId);
    }

    // ─── Pay ─────────────────────────────────────────────────────────────

    /// @notice Pay an invoice in the exact required token.
    ///         Accumulates partial payments — call multiple times for
    ///         subscription-style invoices.
    function pay(bytes32 invoiceId, uint256 inputAmount) external {
        Invoice storage inv = _validateInvoice(invoiceId);

        uint256 remaining = inv.amount - inv.amountPaid;
        uint256 paying    = inputAmount > remaining ? remaining : inputAmount;

        uint256 fee    = (paying * feeBps) / 10_000;
        uint256 net    = paying - fee;

        require(IERC20Pay(inv.token).transferFrom(msg.sender, address(this), paying),
                "Gateway: transfer failed");

        inv.amountPaid      += paying;
        inv.lastPayer        = msg.sender;
        inv.lastInputAmount  = paying;
        inv.lastInputToken   = inv.token;

        merchantBalances[inv.merchantId][inv.token] += net;
        if (fee > 0) _sendToken(inv.token, treasury, fee);

        if (inv.amountPaid >= inv.amount) {
            inv.status = InvoiceStatus.Paid;
            paidAt[invoiceId] = block.timestamp;
        }

        emit InvoicePaid(invoiceId, inv.merchantId, msg.sender,
                         inv.token, paying, paying, fee);
    }

    /// @notice Pay an invoice using any token — auto-swapped via ZbxRouter.
    ///         Customer approves `inputToken` for `maxAmountIn`; the router
    ///         swaps exactly enough to cover the invoice amount.
    ///
    /// @param  invoiceId   Invoice to pay.
    /// @param  inputToken  Token the customer is paying with.
    /// @param  maxAmountIn Maximum the customer is willing to spend.
    /// @param  router      ZbxRouter address (validated against DEX factory).
    /// @param  path        Swap path: [inputToken, ..., invoiceToken].
    // SOL-07 (MEDIUM): nonReentrant added — payWithConvert calls an external
    // router (swapTokensForExactTokens) and then _sendToken for the refund.
    // A malicious router could re-enter pay/payWithConvert and double-credit
    // the invoice or steal the input token refund before state is updated.
    function payWithConvert(
        bytes32           invoiceId,
        address           inputToken,
        uint256           maxAmountIn,
        address           router,
        address[] calldata path
    ) external nonReentrant {
        Invoice storage inv = _validateInvoice(invoiceId);
        require(router != address(0),    "Gateway: zero router");
        require(path.length >= 2,        "Gateway: bad path");
        require(path[path.length-1] == inv.token, "Gateway: path end != invoice token");
        require(path[0] == inputToken,   "Gateway: path start != inputToken");

        uint256 remaining = inv.amount - inv.amountPaid;

        // Pull max from user; router will refund unused amount.
        require(IERC20Pay(inputToken).transferFrom(msg.sender, address(this), maxAmountIn),
                "Gateway: input transfer failed");
        require(IERC20Pay(inputToken).approve(router, maxAmountIn),
                "Gateway: approve failed");

        uint256 balBefore = IERC20Pay(inv.token).balanceOf(address(this));
        uint256[] memory amounts = IZbxRouterPay(router).swapTokensForExactTokens(
            remaining, maxAmountIn, path, address(this), block.timestamp + 300
        );
        uint256 balAfter  = IERC20Pay(inv.token).balanceOf(address(this));
        uint256 received  = balAfter - balBefore;
        if (received < remaining) revert InsufficientOutput();

        // Refund unused input
        uint256 used    = amounts[0];
        uint256 refund  = maxAmountIn - used;
        if (refund > 0) _sendToken(inputToken, msg.sender, refund);

        uint256 fee = (received * feeBps) / 10_000;
        uint256 net = received - fee;

        inv.amountPaid      += received;
        inv.lastPayer        = msg.sender;
        inv.lastInputAmount  = used;
        inv.lastInputToken   = inputToken;
        if (inv.amountPaid >= inv.amount) {
            inv.status = InvoiceStatus.Paid;
            paidAt[invoiceId] = block.timestamp;
        }

        merchantBalances[inv.merchantId][inv.token] += net;
        if (fee > 0) _sendToken(inv.token, treasury, fee);

        emit InvoicePaid(invoiceId, inv.merchantId, msg.sender,
                         inputToken, used, received, fee);
    }

    // ─── Refund ───────────────────────────────────────────────────────────

    /// @notice Refund a paid invoice.  Merchant must call within REFUND_WINDOW.
    ///         The customer receives their original input amount back.
    ///         Merchant balance is debited.
    function refund(bytes32 invoiceId) external {
        Invoice storage inv = invoices[invoiceId];
        if (inv.amount == 0)                       revert InvoiceNotFound();
        if (merchants[inv.merchantId].owner != msg.sender) revert NotMerchant();
        if (inv.status != InvoiceStatus.Paid)      revert NotPaid();
        if (block.timestamp > paidAt[invoiceId] + REFUND_WINDOW)
                                                   revert RefundWindowPassed();

        uint256 refundAmt = inv.amountPaid;
        address refundTok = inv.lastInputToken;
        address payer     = inv.lastPayer;

        // SEC-2026-05-09 Pass-15 (HIGH-S01 / Pass-12 Tier-2 PG-refund):
        // Pre-fix `refund()` paid the customer in `lastInputToken`
        // (whatever token they originally swapped from) but debited
        // the merchant in `inv.token` (the invoice's quote token). If
        // the merchant held a high-value `inv.token` balance and the
        // payer used a low-value `lastInputToken`, an attacker could
        // pay 1 USDC, refund into 1 ETH, and drain merchant inventory.
        // Cross-token refunds are now disabled at the contract level —
        // refunds happen in the invoice's quote token only. Swap
        // routers must execute reverse-conversions off the gateway.
        require(refundTok == address(0) || refundTok == inv.token,
                "PG: cross-token refund disabled");

        inv.status     = InvoiceStatus.Cancelled;
        inv.amountPaid = 0;

        // Debit merchant balance (in invoice token)
        uint256 net = refundAmt - (refundAmt * feeBps / 10_000);
        merchantBalances[inv.merchantId][inv.token] -= net;

        _sendToken(inv.token, payer, refundAmt);
        emit Refunded(invoiceId, payer, refundAmt);
    }

    // ─── Merchant withdrawals ─────────────────────────────────────────────

    /// @notice Withdraw accumulated invoice payments.
    /// @param  merchantId  Your merchantId.
    /// @param  token       Token to withdraw.
    function withdraw(bytes32 merchantId, address token) external {
        Merchant storage m = merchants[merchantId];
        if (m.owner != msg.sender) revert NotMerchant();

        uint256 amount = merchantBalances[merchantId][token];
        if (amount == 0) return;

        merchantBalances[merchantId][token] = 0;
        _sendToken(token, m.payoutAddress, amount);

        emit MerchantWithdrawal(merchantId, token, amount);
    }

    // ─── View helpers ─────────────────────────────────────────────────────

    /// @notice True if the invoice is open and not yet expired.
    function isPayable(bytes32 invoiceId) external view returns (bool) {
        Invoice storage inv = invoices[invoiceId];
        return inv.status == InvoiceStatus.Pending
            && block.timestamp <= inv.expiresAt
            && inv.amountPaid < inv.amount;
    }

    /// @notice Remaining amount needed to fully pay an invoice.
    function amountDue(bytes32 invoiceId) external view returns (uint256) {
        Invoice storage inv = invoices[invoiceId];
        if (inv.amountPaid >= inv.amount) return 0;
        return inv.amount - inv.amountPaid;
    }

    // ─── Admin ────────────────────────────────────────────────────────────

    function setFeeBps(uint256 bps) external {
        if (msg.sender != gatewayOwner) revert Unauthorised();
        if (bps > MAX_FEE_BPS)          revert InvalidFeeBps();
        feeBps = bps;
        emit ProtocolFeeUpdated(bps);
    }

    function setTreasury(address t) external {
        if (msg.sender != gatewayOwner) revert Unauthorised();
        if (t == address(0))            revert ZeroAddress();
        treasury = t;
        emit TreasuryUpdated(t);
    }

    // ─── Internal ─────────────────────────────────────────────────────────

    function _validateInvoice(bytes32 invoiceId)
        private view returns (Invoice storage inv)
    {
        inv = invoices[invoiceId];
        if (inv.amount == 0)                      revert InvoiceNotFound();
        if (inv.status == InvoiceStatus.Cancelled) revert InvoiceCancelled();
        if (inv.status == InvoiceStatus.Paid)      revert InvoiceAlreadyPaid();
        if (block.timestamp > inv.expiresAt)       revert InvoiceExpired();
    }

    function _sendToken(address token, address to, uint256 amount) private {
        if (amount == 0) return;
        require(IERC20Pay(token).transfer(to, amount), "Gateway: send failed");
    }
}
