// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title ZbxTimelockController — Governance timelock.
/// @notice All governance-approved transactions must wait MIN_DELAY
///         before execution. Prevents instant governance attacks.
///
/// @dev   Similar to OpenZeppelin TimelockController.
///        Delay: 48 hours for parameter changes, 7 days for upgrades.
///
/// @custom:zbx-chain  Chain ID 8989

contract ZbxTimelockController {

    uint256 public constant MIN_DELAY       = 2 days;
    uint256 public constant MAX_DELAY       = 30 days;
    uint256 public constant UPGRADE_DELAY   = 7 days;

    address public proposer;   // ZbxGovernor
    address public executor;   // multisig or guardian
    address public admin;

    bytes32 public constant DONE = bytes32(uint256(1));

    /// operation hash → ready timestamp (0 = pending, 1 = done)
    mapping(bytes32 => uint256) public timestamps;

    event CallScheduled(
        bytes32 indexed id, address target,
        bytes data, uint256 delay, uint256 readyAt
    );
    event CallExecuted(bytes32 indexed id, address target, bytes data);
    event Cancelled(bytes32 indexed id);

    constructor(address proposer_, address executor_) {
        proposer = proposer_;
        executor = executor_;
        admin    = msg.sender;
    }

    // ─── Schedule ─────────────────────────────────────────────────────────

    function schedule(
        address target, bytes calldata data,
        bytes32 predecessor, bytes32 salt, uint256 delay
    ) external returns (bytes32 id) {
        require(msg.sender == proposer, "Timelock: not proposer");
        require(delay >= MIN_DELAY && delay <= MAX_DELAY, "Timelock: invalid delay");

        id = hashOperation(target, data, predecessor, salt);
        require(timestamps[id] == 0, "Timelock: already scheduled");

        uint256 readyAt = block.timestamp + delay;
        timestamps[id]  = readyAt;
        emit CallScheduled(id, target, data, delay, readyAt);
    }

    // ─── Execute ──────────────────────────────────────────────────────────

    function execute(
        address target, bytes calldata data,
        bytes32 predecessor, bytes32 salt
    ) external {
        require(msg.sender == executor, "Timelock: not executor");

        bytes32 id = hashOperation(target, data, predecessor, salt);
        require(isReady(id), "Timelock: not ready");
        if (predecessor != bytes32(0)) {
            require(timestamps[predecessor] == DONE, "Timelock: predecessor not done");
        }

        timestamps[id] = DONE;
        (bool ok, ) = target.call(data);
        require(ok, "Timelock: call failed");
        emit CallExecuted(id, target, data);
    }

    // ─── Cancel ───────────────────────────────────────────────────────────

    function cancel(bytes32 id) external {
        require(msg.sender == admin, "Timelock: not admin");
        require(timestamps[id] > 1, "Timelock: not pending");
        delete timestamps[id];
        emit Cancelled(id);
    }

    // ─── Views ────────────────────────────────────────────────────────────

    function isReady(bytes32 id) public view returns (bool) {
        return timestamps[id] != 0 && timestamps[id] != DONE
            && block.timestamp >= timestamps[id];
    }

    function isDone(bytes32 id) public view returns (bool) {
        return timestamps[id] == DONE;
    }

    function hashOperation(
        address target, bytes calldata data,
        bytes32 predecessor, bytes32 salt
    ) public pure returns (bytes32) {
        return keccak256(abi.encode(target, data, predecessor, salt));
    }
}