// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { IZBXGov }     from "./interfaces/IZBXGov.sol";
import { IZbxTimelock } from "./interfaces/IZbxTimelock.sol";

/// @title ZbxGovernor — On-chain governance for Zebvix Chain protocol.
/// @notice ZBXGov token holders submit and vote on protocol proposals.
///         Passed proposals are queued in ZbxTimelock and executed after delay.
///
/// @dev   Governance parameters (adjustable by governance itself):
///          - Voting delay:  1 block  (time from proposal → voting starts)
///          - Voting period: 50400 blocks (~7 days at 12s per block)
///          - Quorum:        4% of total ZBXGov supply
///          - Proposal threshold: 100 ZBXGov (prevents spam)
///
///        Vote types: Against (0), For (1), Abstain (2)
///
/// @custom:zbx-chain  Chain ID 8989

contract ZbxGovernor {

    // ─── Governance parameters ────────────────────────────────────────────

    uint256 public votingDelay    = 1;        // blocks
    uint256 public votingPeriod   = 50_400;   // blocks (~7 days)
    uint256 public proposalThreshold = 100e18; // ZBXGov needed to propose
    uint256 public quorumNumerator   = 4;      // 4% of total supply

    // ─── Proposal state ───────────────────────────────────────────────────

    enum ProposalState { Pending, Active, Cancelled, Defeated, Succeeded, Queued, Expired, Executed }

    struct Proposal {
        uint256 id;
        address proposer;
        address[] targets;
        uint256[] values;
        bytes[]   calldatas;
        string    description;
        uint256   startBlock;
        uint256   endBlock;
        uint256   forVotes;
        uint256   againstVotes;
        uint256   abstainVotes;
        bool      cancelled;
        bool      executed;
    }

    struct Receipt {
        bool    hasVoted;
        uint8   support;     // 0=Against, 1=For, 2=Abstain
        uint256 votes;
    }

    // ─── State ────────────────────────────────────────────────────────────

    address public immutable token;      // ZBXGov token
    address public immutable timelock;   // ZbxTimelock

    uint256 public proposalCount;
    mapping(uint256 => Proposal) public proposals;
    mapping(uint256 => mapping(address => Receipt)) public receipts;

    // ─── S37-governor-timelock (AUDIT C-19 closure) ────────────────────────
    //
    // proposalEta[id] is the Unix timestamp the timelock will allow the
    // proposal's actions to execute. Set in `queue()`, read in `execute()`
    // / `cancel()` / `state()`. A value of 0 means "never queued" and is
    // the marker used by `state()` to distinguish Succeeded from Queued
    // / Expired.
    //
    // Why an eta is recorded per-proposal even though ZbxTimelock already
    // tracks each (target, value, sig, data, eta) txHash:
    //   1. txHash uniqueness: if two distinct proposals queue identical
    //      (target, value, data) at the same block, txHash collides and
    //      one execute would revert TxNotQueued. We break the collision
    //      by adding `proposalId` (a strictly-monotone uint256) to eta,
    //      so eta values differ across proposals queued in the same
    //      block. See `queue()` for the formula and proof.
    //   2. execute() needs to recompute the same eta to derive the same
    //      txHash; storing it once avoids re-deriving from
    //      block.timestamp (which would differ).
    mapping(uint256 => uint256) public proposalEta;

    // ─── Events ───────────────────────────────────────────────────────────

    event ProposalCreated(
        uint256 indexed id,
        address proposer,
        address[] targets,
        uint256[] values,
        bytes[]   calldatas,
        string    description,
        uint256   startBlock,
        uint256   endBlock
    );
    event VoteCast(address indexed voter, uint256 indexed proposalId, uint8 support, uint256 votes);
    event ProposalQueued(uint256 indexed id, uint256 eta);
    event ProposalExecuted(uint256 indexed id);
    event ProposalCancelled(uint256 indexed id);
    event ParamUpdated(string param, uint256 oldValue, uint256 newValue);

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor(address token_, address timelock_) {
        require(token_    != address(0), "Governor: zero token");
        require(timelock_ != address(0), "Governor: zero timelock");
        token    = token_;
        timelock = timelock_;
    }

    // ─── Deployment Verification ──────────────────────────────────────────

    /// @notice Verify that this Governor is correctly wired as the timelock admin.
    ///         Call this immediately after deployment to confirm the setup before
    ///         any governance actions are taken.
    ///
    /// @dev    OPERATOR-03 fix: The timelock's admin must be set to address(this)
    ///         after deploy.  Typical sequence:
    ///           1. Deploy ZbxTimelock (admin = deployer EOA initially)
    ///           2. Deploy ZbxGovernor(token, timelock)
    ///           3. timelock.setPendingAdmin(address(governor))
    ///           4. governor.acceptTimelockAdmin()   ← sets timelock.admin = governor
    ///           5. Call governor.verifySetup()       ← returns (true, "")
    ///
    /// @return ok     `true` when all invariants hold; `false` on misconfiguration.
    /// @return issue  Empty string when ok; human-readable error description otherwise.
    function verifySetup() external view returns (bool ok, string memory issue) {
        address timelockAdmin = IZbxTimelock(timelock).admin();
        if (timelockAdmin != address(this)) {
            return (
                false,
                "Governor: timelock.admin != address(this). "
                "Call timelock.setPendingAdmin(governor) then governor.acceptTimelockAdmin()."
            );
        }
        if (token == address(0)) {
            return (false, "Governor: token is zero address");
        }
        return (true, "");
    }

    /// @notice Accept the pending admin role on the timelock.
    ///         Call this after `timelock.setPendingAdmin(address(this))`.
    ///         After this call `verifySetup()` should return (true, "").
    function acceptTimelockAdmin() external {
        IZbxTimelock(timelock).acceptAdmin();
    }

    // ─── Propose ──────────────────────────────────────────────────────────

    function propose(
        address[] calldata targets,
        uint256[] calldata values,
        bytes[]   calldata calldatas,
        string    calldata description
    ) external returns (uint256 id) {
        require(targets.length > 0,                 "Governor: no actions");
        require(targets.length == values.length,    "Governor: length mismatch");
        require(targets.length == calldatas.length, "Governor: length mismatch");
        require(targets.length <= 10,               "Governor: too many actions");

        uint256 votes = _getVotes(msg.sender, block.number - 1);
        require(votes >= proposalThreshold, "Governor: below proposal threshold");

        id = ++proposalCount;
        proposals[id] = Proposal({
            id:           id,
            proposer:     msg.sender,
            targets:      targets,
            values:       values,
            calldatas:    calldatas,
            description:  description,
            startBlock:   block.number + votingDelay,
            endBlock:     block.number + votingDelay + votingPeriod,
            forVotes:     0,
            againstVotes: 0,
            abstainVotes: 0,
            cancelled:    false,
            executed:     false
        });

        emit ProposalCreated(id, msg.sender, targets, values, calldatas, description,
                             block.number + votingDelay, block.number + votingDelay + votingPeriod);
    }

    // ─── Vote ─────────────────────────────────────────────────────────────

    function castVote(uint256 proposalId, uint8 support) external returns (uint256) {
        return _castVote(proposalId, msg.sender, support);
    }

    function _castVote(uint256 proposalId, address voter, uint8 support) internal returns (uint256 weight) {
        require(state(proposalId) == ProposalState.Active, "Governor: not active");
        require(support <= 2,                              "Governor: invalid vote type");
        require(!receipts[proposalId][voter].hasVoted,     "Governor: already voted");

        Proposal storage p = proposals[proposalId];
        // S22a off-by-one: state() activates at block.number == startBlock,
        // but getPriorVotes requires the queried block to be strictly past.
        // The snapshot is therefore the block BEFORE voting opens (startBlock - 1).
        weight = _getVotes(voter, p.startBlock - 1);

        receipts[proposalId][voter] = Receipt({ hasVoted: true, support: support, votes: weight });

        if      (support == 0) p.againstVotes += weight;
        else if (support == 1) p.forVotes     += weight;
        else                   p.abstainVotes  += weight;

        emit VoteCast(voter, proposalId, support, weight);
    }

    // ─── Queue & Execute (S37-governor-timelock — AUDIT C-19 closure) ────
    //
    // Pre-S37, queue() and execute() were emit-only stubs:
    //     emit ProposalQueued(proposalId, block.timestamp + 172800);
    //     emit ProposalExecuted(proposalId);
    // with the inline comment "Real impl: IZbxTimelock(timelock).scheduleBatch(...)"
    // explicitly admitting the wiring was missing. Result: a passed
    // proposal flipped Succeeded → Queued (well, would-be-Queued — the
    // pre-S37 state() never returned Queued either, so execute()'s own
    // require would fail, making the path doubly-broken) → Executed in
    // Governor state but produced ZERO on-chain effect against
    // ZbxBridge / ZbxLendingPool / BridgeVault / BridgeMultisig
    // (Governor is post-S3-cutover admin of all four).
    //
    // S37 wires the per-action loop against the Compound-style per-tx
    // ZbxTimelock interface. ZbxTimelock has no `scheduleBatch` —
    // batching is done client-side here. msg.value handling: Option A
    // (caller-funded) — caller of `execute()` MUST supply
    // sum(values[]) up-front; any deviation reverts (sum > msg.value
    // reverts inside the loop on overdraw, sum < msg.value reverts on
    // the post-loop accounting check so no ETH gets stranded in the
    // Governor).

    /// @notice Queue every action of a Succeeded proposal in ZbxTimelock.
    ///         Each action is queued as a separate timelock tx with the
    ///         same `eta`. Caller does not need any voting weight; this
    ///         is permissionless once the proposal has Succeeded.
    function queue(uint256 proposalId) external {
        require(state(proposalId) == ProposalState.Succeeded, "Governor: not succeeded");
        Proposal storage p = proposals[proposalId];

        // Read delay() dynamically so an in-protocol `setDelay` proposal
        // (executed via this same Governor) updates queueing behaviour
        // without redeployment.
        //
        // proposalId-uniquification: ZbxTimelock identifies queued txs
        // by keccak256(abi.encode(target, value, signature, data, eta)).
        // Two distinct proposals with identical (target, value, "", data)
        // queued in the same block would otherwise collide on a single
        // txHash, causing the second `executeTransaction` to revert
        // `TxNotQueued`. proposalId is strictly monotone (++proposalCount)
        // so adding it to eta yields a different eta — and therefore a
        // different txHash — for every collision-eligible proposal pair.
        // The eta stays >= block.timestamp + delay (timelock requirement).
        uint256 eta = block.timestamp + IZbxTimelock(timelock).delay() + proposalId;
        proposalEta[proposalId] = eta;

        uint256 n = p.targets.length;
        for (uint256 i = 0; i < n; i++) {
            // Empty signature string — `data` is already pre-encoded
            // calldata (selector + args), see ZbxTimelock.executeTransaction
            // line 185 for the branch that takes `data` verbatim when
            // signature is empty.
            IZbxTimelock(timelock).queueTransaction(
                p.targets[i], p.values[i], "", p.calldatas[i], eta
            );
        }

        emit ProposalQueued(proposalId, eta);
    }

    /// @notice Execute every action of a Queued proposal via ZbxTimelock.
    ///         Caller forwards exactly `sum(values[i])` ETH; the timelock
    ///         enforces the `eta`/GRACE window per-action.
    /// SEC-2026-05-09 Pass-15 (HIGH-S10): explicit reentrancy lock.
    /// Pre-fix `execute()` relied on the `p.executed = true` CEI
    /// flag at the top of the function to block re-entry. That works
    /// for re-entry through THIS proposal but does NOT block a
    /// proposal that targets `Governor.execute(otherProposalId)` and
    /// chains through the timelock — `_executing` here forces a
    /// global single-flight, which is what governance call graphs
    /// actually need.
    bool private _executing;
    modifier nonReentrantExec() {
        require(!_executing, "Governor: reentrant execute");
        _executing = true;
        _;
        _executing = false;
    }

    function execute(uint256 proposalId) external payable nonReentrantExec {
        require(state(proposalId) == ProposalState.Queued, "Governor: not queued");
        Proposal storage p = proposals[proposalId];
        uint256 eta = proposalEta[proposalId];

        // Mark executed up-front (CEI pattern — prevents a re-entrant
        // executeTransaction → execute call from double-running). The
        // proposal is no longer in Queued state on a re-entry, so
        // state() returns Executed and the require above traps the
        // re-entry.
        p.executed = true;

        uint256 n = p.targets.length;
        uint256 sent = 0;
        for (uint256 i = 0; i < n; i++) {
            IZbxTimelock(timelock).executeTransaction{value: p.values[i]}(
                p.targets[i], p.values[i], "", p.calldatas[i], eta
            );
            // Solidity 0.8 checked arithmetic — overflow is not possible
            // here because n <= 10 (propose() require) and values[i] are
            // bounded by msg.value above, but keep the explicit add for
            // gas/audit clarity.
            sent += p.values[i];
        }

        // Tighter than Compound: Compound silently accepts excess
        // msg.value (it gets stuck in the Timelock); we reject so no
        // ETH is ever stranded in either contract.
        require(msg.value == sent, "Governor: msg.value != sum(values)");

        emit ProposalExecuted(proposalId);
    }

    /// @notice Cancel a proposal.
    ///
    /// @dev    NEW-HIGH-05 fix (2026-05-05) — anti-griefing cancel.
    ///
    ///         Allowed if EITHER:
    ///           (a) the caller is the original proposer, OR
    ///           (b) the proposer's current voting power has dropped below
    ///               `proposalThreshold` (mirrors Compound Governor Bravo
    ///               anti-griefing logic).
    ///
    ///         Without condition (b) a proposer who loses tokens after
    ///         submitting a harmful proposal can keep it alive indefinitely:
    ///         they need only retain 1 wei of ZBXGov to block cancellation,
    ///         while the community has no recourse until the proposal expires.
    ///
    ///         Applies to Pending, Active, and Queued states.  When Queued,
    ///         every per-action timelock entry is also cancelled so a future
    ///         operator-level `executeTransaction` cannot fire the cancelled
    ///         proposal piecemeal.
    function cancel(uint256 proposalId) external {
        Proposal storage p = proposals[proposalId];

        // NEW-HIGH-05: allow anyone to cancel if proposer is below threshold.
        uint256 proposerVotes = _getVotes(p.proposer, block.number - 1);
        require(
            msg.sender == p.proposer || proposerVotes < proposalThreshold,
            "Governor: not proposer or proposer above threshold"
        );

        ProposalState s = state(proposalId);
        require(
            s == ProposalState.Pending ||
            s == ProposalState.Active  ||
            s == ProposalState.Queued,
            "Governor: cannot cancel"
        );

        if (s == ProposalState.Queued) {
            uint256 eta = proposalEta[proposalId];
            uint256 n = p.targets.length;
            for (uint256 i = 0; i < n; i++) {
                IZbxTimelock(timelock).cancelTransaction(
                    p.targets[i], p.values[i], "", p.calldatas[i], eta
                );
            }
        }

        p.cancelled = true;
        emit ProposalCancelled(proposalId);
    }

    // ─── State ────────────────────────────────────────────────────────────

    /// @notice Resolve the lifecycle state of a proposal.
    /// @dev    S37 added the Queued / Expired branches. Pre-S37 this
    ///         function never returned ProposalState.Queued, so the
    ///         require in execute() (`state(id) == Queued`) was
    ///         unreachable — execute() was effectively dead code on
    ///         top of being a no-op.
    function state(uint256 proposalId) public view returns (ProposalState) {
        Proposal storage p = proposals[proposalId];
        require(p.id != 0,     "Governor: unknown proposal");
        if (p.cancelled)       return ProposalState.Cancelled;
        if (p.executed)        return ProposalState.Executed;
        if (block.number < p.startBlock) return ProposalState.Pending;
        if (block.number <= p.endBlock)  return ProposalState.Active;

        // Voting period ended — check tally first.
        if (!(_quorumReached(proposalId) && _voteSucceeded(proposalId))) {
            return ProposalState.Defeated;
        }

        // S37 (C-19): fork on whether `queue()` has been called.
        // proposalEta[id] == 0 ↔ not yet queued ↔ Succeeded.
        // proposalEta[id] != 0 ↔ queued; check GRACE window for Expired.
        uint256 eta = proposalEta[proposalId];
        if (eta == 0) return ProposalState.Succeeded;

        // Expired iff we've passed the timelock's GRACE_PERIOD past eta.
        // executeTransaction reverts TxStale beyond that point, so
        // returning Expired here gives off-chain UIs a clean signal
        // before the on-chain revert.
        if (block.timestamp > eta + IZbxTimelock(timelock).GRACE_PERIOD()) {
            return ProposalState.Expired;
        }
        return ProposalState.Queued;
    }

    // ─── Quorum ───────────────────────────────────────────────────────────

    function quorum(uint256 blockNumber) public view returns (uint256) {
        uint256 supply = _totalSupplyAt(blockNumber);
        return supply * quorumNumerator / 100;
    }

    function _quorumReached(uint256 id) internal view returns (bool) {
        Proposal storage p = proposals[id];
        // S22a: snapshot is startBlock - 1 (see _castVote comment).
        return (p.forVotes + p.abstainVotes) >= quorum(p.startBlock - 1);
    }

    function _voteSucceeded(uint256 id) internal view returns (bool) {
        Proposal storage p = proposals[id];
        return p.forVotes > p.againstVotes;
    }

    // ─── Governance parameter updates (via governance itself) ─────────────

    function updateVotingDelay(uint256 newDelay) external {
        require(msg.sender == address(this), "Governor: only governance");
        emit ParamUpdated("votingDelay", votingDelay, newDelay);
        votingDelay = newDelay;
    }

    function updateVotingPeriod(uint256 newPeriod) external {
        require(msg.sender == address(this), "Governor: only governance");
        require(newPeriod >= 5760, "Governor: period too short"); // min 1 day
        emit ParamUpdated("votingPeriod", votingPeriod, newPeriod);
        votingPeriod = newPeriod;
    }

    function updateProposalThreshold(uint256 newThreshold) external {
        require(msg.sender == address(this), "Governor: only governance");
        emit ParamUpdated("proposalThreshold", proposalThreshold, newThreshold);
        proposalThreshold = newThreshold;
    }

    // ─── Vote / supply queries (delegate to ZBXGov checkpoints, S22a) ────
    //
    // S22a wired the previously-stubbed _getVotes / _totalSupplyAt to the
    // real ZBXGov checkpoint storage via IZBXGov. Pre-S22a these stubs
    // returned 0, meaning:
    //   * every proposal would FAIL quorum (4% of 0 = 0 votes-for required,
    //     but 0 forVotes accumulated → _voteSucceeded false → Defeated), AND
    //   * any address with 0 ZBXGov could meet the proposal threshold
    //     (`votes >= proposalThreshold` was `0 >= 100e18` = false, so
    //     propose() actually correctly blocked spam — but only by accident,
    //     because the stub returned 0 which is < 100e18).
    // Net pre-S22a effect: governance was inert (no proposals could pass
    // and none could be created either). S22a closes both halves.
    //
    // Off-by-one note: callers (_castVote, _quorumReached) pass
    // `p.startBlock - 1` (the snapshot block, strictly past), NOT
    // p.startBlock itself, because state() considers `block.number ==
    // startBlock` Active and getPriorVotes requires `blockNumber <
    // block.number`. propose() correctly uses `block.number - 1` already.

    function _getVotes(address account, uint256 blockNumber) internal view returns (uint256) {
        return IZBXGov(token).getPriorVotes(account, blockNumber);
    }

    function _totalSupplyAt(uint256 blockNumber) internal view returns (uint256) {
        return IZBXGov(token).totalSupplyAt(blockNumber);
    }

    // ─── Views ────────────────────────────────────────────────────────────

    function getReceipt(uint256 proposalId, address voter) external view returns (Receipt memory) {
        return receipts[proposalId][voter];
    }

    // ─── proposalVotes (NEW-MED-03 fix) ───────────────────────────────────

    /// @notice Return the current vote tally for a proposal.
    ///
    /// @dev    NEW-MED-03 fix (2026-05-05).  The IZbxGovernor ABI requires
    ///         this selector.  Previously the data existed on the Proposal
    ///         struct but was not exposed as an external view, so tooling
    ///         (Tally, Snapshot, Boardroom) and cross-contract integrators
    ///         had no standard way to query live vote counts.
    ///
    ///         Exposes forVotes, againstVotes, abstainVotes directly from the
    ///         storage struct — no computation, no rounding risk.
    function proposalVotes(uint256 proposalId)
        external
        view
        returns (
            uint256 againstVotes,
            uint256 forVotes,
            uint256 abstainVotes
        )
    {
        Proposal storage p = proposals[proposalId];
        require(p.id != 0, "Governor: unknown proposal");
        return (p.againstVotes, p.forVotes, p.abstainVotes);
    }

    // ─── EIP-165 supportsInterface (S21 + NEW-MED-03) ─────────────────────

    function supportsInterface(bytes4 interfaceId) external pure returns (bool) {
        return interfaceId == 0x01ffc9a7;   // EIP-165 itself
    }
}