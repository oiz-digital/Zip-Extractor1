// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxGameEscrow — Interface for ZbxGameEscrow wager/prize escrow for on-chain games.
interface IZbxGameEscrow {
    enum EscrowStatus { Pending, Released, Refunded, Disputed }

    struct Escrow {
        address  depositor;
        address  beneficiary;
        address  arbiter;
        address  token;
        uint256  amount;
        EscrowStatus status;
        uint256  deadline;
    }

    event EscrowCreated(bytes32 indexed escrowId, address depositor, address beneficiary, uint256 amount);
    event EscrowReleased(bytes32 indexed escrowId, address indexed beneficiary, uint256 amount);
    event EscrowRefunded(bytes32 indexed escrowId, address indexed depositor, uint256 amount);
    event DisputeRaised(bytes32 indexed escrowId);
    event DisputeResolved(bytes32 indexed escrowId, address indexed winner);

    function createEscrow(address beneficiary, address arbiter, address token, uint256 amount, uint256 duration) external payable returns (bytes32 escrowId);
    function release(bytes32 escrowId) external;
    function refund(bytes32 escrowId) external;
    function raiseDispute(bytes32 escrowId) external;
    function resolveDispute(bytes32 escrowId, bool releaseToBeneficiary) external;
    function getEscrow(bytes32 escrowId) external view returns (Escrow memory);
}
