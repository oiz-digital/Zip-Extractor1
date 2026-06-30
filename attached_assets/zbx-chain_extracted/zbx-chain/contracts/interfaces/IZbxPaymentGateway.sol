// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxPaymentGateway — Interface for ZbxPaymentGateway merchant payment processing.
interface IZbxPaymentGateway {
    struct Invoice {
        address merchant;
        address token;
        uint256 amount;
        uint256 expiry;
        bool    paid;
        bool    cancelled;
        bool    refunded;
    }

    event InvoiceCreated(bytes32 indexed invoiceId, address indexed merchant, address token, uint256 amount, uint256 expiry);
    event InvoicePaid(bytes32 indexed invoiceId, address indexed payer);
    event InvoiceRefunded(bytes32 indexed invoiceId, address indexed recipient);
    event MerchantRegistered(address indexed merchant);
    event MerchantDeregistered(address indexed merchant);

    error NotMerchant();
    error InvoiceNotFound();
    error InvoiceExpired();
    error InvoiceAlreadyPaid();
    error InvoiceCancelled();
    error RefundWindowPassed();

    function registerMerchant() external;
    function deregisterMerchant() external;
    function createInvoice(address token, uint256 amount, uint256 expirySeconds) external returns (bytes32 invoiceId);
    function pay(bytes32 invoiceId) external payable;
    function refund(bytes32 invoiceId) external;
    function cancelInvoice(bytes32 invoiceId) external;
    function getInvoice(bytes32 invoiceId) external view returns (Invoice memory);
    function isMerchant(address addr) external view returns (bool);
}
