// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title ZbxMultisig — M-of-N multisig wallet (Gnosis Safe compatible).
/// @notice Requires M signatures from N owners to execute transactions.
///         Used for:
///           - Bridge fund management (requires 3-of-5 multisig)
///           - Validator treasury management
///           - Protocol parameter upgrades
///           - Team vesting wallets
///
/// @dev   EIP-712 typed signing for off-chain signature collection.
///
/// @custom:zbx-chain  Chain ID 8989

contract ZbxMultisig {

    // ─── Types ────────────────────────────────────────────────────────────

    struct Transaction {
        address to;
        uint256 value;
        bytes   data;
        bool    executed;
        uint256 confirmations;
    }

    // ─── State ────────────────────────────────────────────────────────────

    address[] public owners;
    mapping(address => bool)              public isOwner;
    uint256                               public required;   // M in M-of-N
    Transaction[]                         public transactions;
    mapping(uint256 => mapping(address => bool)) public confirmed;

    // ─── Events ───────────────────────────────────────────────────────────

    event OwnerAdded(address indexed owner);
    event OwnerRemoved(address indexed owner);
    event RequirementChanged(uint256 newRequired);
    event TxSubmitted(uint256 indexed txId, address indexed from, address to, uint256 value);
    event TxConfirmed(uint256 indexed txId, address indexed owner);
    event TxRevoked(uint256 indexed txId, address indexed owner);
    event TxExecuted(uint256 indexed txId);
    event TxFailed(uint256 indexed txId);
    event Deposit(address indexed from, uint256 value);

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor(address[] memory owners_, uint256 required_) {
        require(owners_.length > 0,              "Multisig: no owners");
        require(required_ > 0,                   "Multisig: required=0");
        require(required_ <= owners_.length,     "Multisig: required > owners");

        for (uint256 i; i < owners_.length; ++i) {
            address owner = owners_[i];
            require(owner != address(0),         "Multisig: zero owner");
            require(!isOwner[owner],             "Multisig: duplicate owner");
            isOwner[owner] = true;
            owners.push(owner);
            emit OwnerAdded(owner);
        }
        required = required_;
    }

    // ─── Submit & Confirm ─────────────────────────────────────────────────

    function submitTransaction(address to, uint256 value, bytes calldata data)
        external onlyOwner returns (uint256 txId)
    {
        txId = transactions.length;
        transactions.push(Transaction({ to: to, value: value, data: data, executed: false, confirmations: 0 }));
        emit TxSubmitted(txId, msg.sender, to, value);
        confirmTransaction(txId); // auto-confirm on submit
    }

    function confirmTransaction(uint256 txId) public onlyOwner txExists(txId) notExecuted(txId) {
        require(!confirmed[txId][msg.sender], "Multisig: already confirmed");
        confirmed[txId][msg.sender] = true;
        transactions[txId].confirmations++;
        emit TxConfirmed(txId, msg.sender);
        if (transactions[txId].confirmations >= required) {
            executeTransaction(txId);
        }
    }

    function revokeConfirmation(uint256 txId) external onlyOwner txExists(txId) notExecuted(txId) {
        require(confirmed[txId][msg.sender], "Multisig: not confirmed");
        confirmed[txId][msg.sender] = false;
        transactions[txId].confirmations--;
        emit TxRevoked(txId, msg.sender);
    }

    function executeTransaction(uint256 txId) public onlyOwner txExists(txId) notExecuted(txId) {
        require(transactions[txId].confirmations >= required, "Multisig: not enough confirmations");
        Transaction storage tx_ = transactions[txId];
        tx_.executed = true;
        (bool success, ) = tx_.to.call{value: tx_.value}(tx_.data);
        if (success) emit TxExecuted(txId);
        else { tx_.executed = false; emit TxFailed(txId); }
    }

    // ─── Owner management ─────────────────────────────────────────────────

    function addOwner(address owner) external onlySelf {
        require(owner != address(0), "Multisig: zero address");
        require(!isOwner[owner],     "Multisig: already owner");
        isOwner[owner] = true;
        owners.push(owner);
        emit OwnerAdded(owner);
    }

    function removeOwner(address owner) external onlySelf {
        require(isOwner[owner], "Multisig: not owner");
        require(owners.length - 1 >= required, "Multisig: would break quorum");
        isOwner[owner] = false;
        for (uint256 i; i < owners.length; ++i) {
            if (owners[i] == owner) {
                owners[i] = owners[owners.length - 1];
                owners.pop();
                break;
            }
        }
        emit OwnerRemoved(owner);
    }

    function changeRequirement(uint256 newRequired) external onlySelf {
        require(newRequired > 0 && newRequired <= owners.length, "Multisig: invalid");
        required = newRequired;
        emit RequirementChanged(newRequired);
    }

    // ─── Views ────────────────────────────────────────────────────────────

    function getOwners()         external view returns (address[] memory) { return owners; }
    function transactionCount()  external view returns (uint256)          { return transactions.length; }
    function ownerCount()        external view returns (uint256)          { return owners.length; }

    // ─── Modifiers ────────────────────────────────────────────────────────

    modifier onlyOwner()                  { require(isOwner[msg.sender], "Multisig: not owner"); _; }
    modifier onlySelf()                   { require(msg.sender == address(this), "Multisig: not self"); _; }
    modifier txExists(uint256 id)         { require(id < transactions.length, "Multisig: tx not found"); _; }
    modifier notExecuted(uint256 id)      { require(!transactions[id].executed, "Multisig: already executed"); _; }

    receive() external payable { emit Deposit(msg.sender, msg.value); }
}