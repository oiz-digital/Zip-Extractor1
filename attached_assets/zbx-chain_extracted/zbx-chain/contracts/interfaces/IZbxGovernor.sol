// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxGovernor — Interface for ZbxGovernor (on-chain governance).
interface IZbxGovernor {
    enum ProposalState {
        Pending, Active, Cancelled, Defeated, Succeeded, Queued, Expired, Executed
    }

    struct Proposal {
        uint256 id;
        address proposer;
        uint256 startBlock;
        uint256 endBlock;
        address[] targets;
        uint256[] values;
        bytes[]   calldatas;
        string    description;
    }

    event ProposalCreated(
        uint256 indexed id, address indexed proposer,
        address[] targets, uint256[] values,
        bytes[] calldatas, string description,
        uint256 startBlock, uint256 endBlock
    );
    event VoteCast(address indexed voter, uint256 indexed proposalId, uint8 support, uint256 weight);
    event ProposalExecuted(uint256 indexed id);
    event ProposalCancelled(uint256 indexed id);

    function propose(
        address[] calldata targets, uint256[] calldata values,
        bytes[] calldata calldatas, string calldata description
    ) external returns (uint256 proposalId);

    function castVote(uint256 proposalId, uint8 support) external returns (uint256 weight);
    function execute(uint256 proposalId) external;
    function cancel(uint256 proposalId) external;
    function queue(uint256 proposalId) external;

    function state(uint256 proposalId)        external view returns (ProposalState);
    function proposalVotes(uint256 proposalId) external view returns (uint256 against, uint256 forVotes, uint256 abstain);
    function quorumNumerator()                 external view returns (uint256);
    function votingPeriod()                    external view returns (uint256);
    function votingDelay()                     external view returns (uint256);
}