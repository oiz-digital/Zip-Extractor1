// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

/// @title ZbxTimelock — governance timelock for founder / multisig ops
/// @author Zebvix Technologies Pvt Ltd
/// @notice Compound-style timelock controller. Founder (or a Gnosis-Safe
///         multisig) holds the admin role; every privileged action — token
///         pause, vault config rotation, reward-rate change, AMM upgrade —
///         must be `queued`, then waited out for `delay` seconds, then
///         `executed`. Emergency `cancel` is allowed at any time.
///
/// @dev    Pattern is a near-faithful port of Compound's `Timelock.sol`:
///             1. admin.queueTransaction(target, value, sig, data, eta)
///             2. wait until block.timestamp >= eta
///             3. admin.executeTransaction(...)  →  target.call(value, data)
///         Anything wired to require `msg.sender == timelock` becomes safely
///         delay-locked, giving users a pre-warned exit window.
contract ZbxTimelock {
    // ---------------------------------------------------------------------
    // Constants
    // ---------------------------------------------------------------------

    uint256 public constant MIN_DELAY        = 6 hours;
    uint256 public constant MAX_DELAY        = 30 days;
    uint256 public constant GRACE_PERIOD     = 14 days;

    // ---------------------------------------------------------------------
    // State
    // ---------------------------------------------------------------------

    address public admin;
    address public pendingAdmin;
    uint256 public delay;

    /// @notice queuedTransactions[txHash] = true if currently queued.
    mapping(bytes32 => bool) public queuedTransactions;

    // ---------------------------------------------------------------------
    // Errors
    // ---------------------------------------------------------------------

    error NotAdmin();
    error NotPendingAdmin();
    error NotSelf();
    error DelayOutOfRange(uint256 d);
    error EtaTooSoon(uint256 eta, uint256 minEta);
    error TxNotQueued(bytes32 txHash);
    error TxNotReady(uint256 nowTs, uint256 eta);
    error TxStale(uint256 nowTs, uint256 staleAfter);
    error CallReverted(bytes returnData);

    // ---------------------------------------------------------------------
    // Events
    // ---------------------------------------------------------------------

    event NewAdmin(address indexed newAdmin);
    event NewPendingAdmin(address indexed newPendingAdmin);
    event NewDelay(uint256 indexed newDelay);
    event QueueTransaction(
        bytes32 indexed txHash,
        address indexed target,
        uint256 value,
        string  signature,
        bytes   data,
        uint256 eta
    );
    event CancelTransaction(
        bytes32 indexed txHash,
        address indexed target,
        uint256 value,
        string  signature,
        bytes   data,
        uint256 eta
    );
    event ExecuteTransaction(
        bytes32 indexed txHash,
        address indexed target,
        uint256 value,
        string  signature,
        bytes   data,
        uint256 eta
    );

    // ---------------------------------------------------------------------
    // Constructor
    // ---------------------------------------------------------------------

    constructor(address _admin, uint256 _delay) {
        require(_admin != address(0), "ZERO_ADDRESS");
        if (_delay < MIN_DELAY || _delay > MAX_DELAY) revert DelayOutOfRange(_delay);
        admin = _admin;
        delay = _delay;
    }

    // ---------------------------------------------------------------------
    // Modifiers
    // ---------------------------------------------------------------------

    modifier onlyAdmin() {
        if (msg.sender != admin) revert NotAdmin();
        _;
    }

    /// @dev `setDelay` and `setPendingAdmin` must be called via the
    ///      timelock itself (i.e. through queue → execute) so that even
    ///      governance changes obey the delay.
    modifier onlySelf() {
        if (msg.sender != address(this)) revert NotSelf();
        _;
    }

    // ---------------------------------------------------------------------
    // Self-only ops (called via queued transaction targeting this contract)
    // ---------------------------------------------------------------------

    function setDelay(uint256 newDelay) external onlySelf {
        if (newDelay < MIN_DELAY || newDelay > MAX_DELAY) revert DelayOutOfRange(newDelay);
        delay = newDelay;
        emit NewDelay(newDelay);
    }

    function setPendingAdmin(address newPendingAdmin) external onlySelf {
        pendingAdmin = newPendingAdmin;
        emit NewPendingAdmin(newPendingAdmin);
    }

    /// @notice The pending admin accepts the role — completes the two-step
    ///         transfer that prevents accidentally locking governance to a
    ///         dead address.
    function acceptAdmin() external {
        if (msg.sender != pendingAdmin) revert NotPendingAdmin();
        admin = pendingAdmin;
        pendingAdmin = address(0);
        emit NewAdmin(admin);
    }

    // ---------------------------------------------------------------------
    // Queue / cancel / execute
    // ---------------------------------------------------------------------

    /// @notice Queue a transaction for execution after `eta`. `signature`
    ///         is an optional human-readable function selector ("foo(uint)")
    ///         for transparent governance UIs; pass the empty string to
    ///         supply pre-encoded calldata via `data` directly.
    function queueTransaction(
        address target,
        uint256 value,
        string calldata signature,
        bytes calldata data,
        uint256 eta
    ) external onlyAdmin returns (bytes32 txHash) {
        if (eta < block.timestamp + delay) revert EtaTooSoon(eta, block.timestamp + delay);
        txHash = keccak256(abi.encode(target, value, signature, data, eta));
        queuedTransactions[txHash] = true;
        emit QueueTransaction(txHash, target, value, signature, data, eta);
    }

    function cancelTransaction(
        address target,
        uint256 value,
        string calldata signature,
        bytes calldata data,
        uint256 eta
    ) external onlyAdmin {
        bytes32 txHash = keccak256(abi.encode(target, value, signature, data, eta));
        if (!queuedTransactions[txHash]) revert TxNotQueued(txHash);
        queuedTransactions[txHash] = false;
        emit CancelTransaction(txHash, target, value, signature, data, eta);
    }

    function executeTransaction(
        address target,
        uint256 value,
        string calldata signature,
        bytes calldata data,
        uint256 eta
    ) external payable onlyAdmin returns (bytes memory returnData) {
        bytes32 txHash = keccak256(abi.encode(target, value, signature, data, eta));
        if (!queuedTransactions[txHash]) revert TxNotQueued(txHash);
        if (block.timestamp < eta) revert TxNotReady(block.timestamp, eta);
        if (block.timestamp > eta + GRACE_PERIOD) revert TxStale(block.timestamp, eta + GRACE_PERIOD);

        queuedTransactions[txHash] = false;

        bytes memory callData = bytes(signature).length == 0
            ? data
            : abi.encodePacked(bytes4(keccak256(bytes(signature))), data);

        (bool ok, bytes memory ret) = target.call{ value: value }(callData);
        if (!ok) revert CallReverted(ret);

        emit ExecuteTransaction(txHash, target, value, signature, data, eta);
        return ret;
    }

    // ---------------------------------------------------------------------
    // ETH receive — funds executed transactions that need msg.value
    // ---------------------------------------------------------------------

    receive() external payable {}
}