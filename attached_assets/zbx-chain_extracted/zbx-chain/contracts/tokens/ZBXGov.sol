// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { ZRC20Base } from "../ZRC20Base.sol";

/// @title ZBXGov — Zebvix Governance Token
/// @notice On-chain governance token for Zebvix Chain protocol decisions.
///         Holders vote on:
///           - Protocol parameter changes (block time, gas limits, fees)
///           - Bridge vault operators
///           - Treasury fund allocations
///           - ZRC-20 standard updates
///           - Validator set changes (v0.2+ BFT consensus)
///
/// @dev    ZBXGov is NON-TRANSFERABLE by default (soulbound to staked ZBX).
///         Governance power is earned by staking native ZBX in ZbxStaking.sol.
///         The staking contract calls `delegate` to assign voting power.
///
///         Supply model:
///           - 1 ZBXGov is issued per 1 ZBX staked.
///           - ZBXGov is burned when ZBX is unstaked.
///           - No market / no trading — purely for governance.
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:ticker     ZBXG
/// @custom:decimals   18
/// @custom:transferable  false (soulbound)
///
/// @custom:snapshots  Vote-weighting snapshots are served by the existing
///                    `getPriorVotes(account, blockNumber)` and
///                    `totalSupplyAt(blockNumber)` checkpoint readers
///                    below — NOT a separate OZ-style ERC20Snapshot
///                    module. See the "Snapshots-via-Votes semantics
///                    (S22b)" section near the total-supply writer for
///                    the full rationale and out-of-scope boundary
///                    (raw-balance airdrops, ERC-5805 IVotes parity).

contract ZBXGov is ZRC20Base {

    // ─── Roles ────────────────────────────────────────────────────────────

    address public owner;
    address public stakingContract;   // only address allowed to mint/burn

    // ─── Voting snapshots ─────────────────────────────────────────────────

    struct Checkpoint {
        uint32  fromBlock;
        uint224 votes;
    }

    mapping(address => Checkpoint[]) private _checkpoints;
    mapping(address => address)      public  delegates;        // delegatee
    mapping(address => uint256)      public  numCheckpoints;

    /// @notice Total-supply checkpoints (S22a) — historical record of
    ///         total ZBXGov supply (= sum of all delegated voting power
    ///         in the soulbound 1:1-staked model). Written on every
    ///         mint/burn so ZbxGovernor.quorum(blockNumber) can compute
    ///         the supply-denominated quorum threshold against a
    ///         strictly-past snapshot block.
    Checkpoint[] private _totalSupplyCheckpoints;

    // ─── Events ───────────────────────────────────────────────────────────

    event DelegateChanged(address indexed delegator, address indexed fromDelegate, address indexed toDelegate);
    event DelegateVotesChanged(address indexed delegate, uint256 previousVotes, uint256 newVotes);
    event StakingContractUpdated(address indexed prev, address indexed next);
    event OwnershipTransferred(address indexed prev, address indexed next);

    // ─── Constructor ──────────────────────────────────────────────────────

    /// @param stakingContract_  ZbxStaking.sol address — only minter/burner.
    constructor(address stakingContract_) ZRC20Base(
        "Zebvix Governance",
        "ZBXG",
        18,
        "ipfs://QmZBXGovLogoXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX"
    ) {
        require(stakingContract_ != address(0), "ZBXGov: zero staking");
        owner           = msg.sender;
        stakingContract = stakingContract_;
    }

    modifier onlyOwner()   { require(msg.sender == owner,           "ZBXGov: not owner");   _; }
    modifier onlyStaking() { require(msg.sender == stakingContract, "ZBXGov: not staking"); _; }

    // ─── Mint / Burn (staking contract only) ─────────────────────────────

    /// @notice Called by ZbxStaking when ZBX is staked — issue governance power.
    function mint(address to, uint256 amount) external onlyStaking {
        _mint(to, amount);
        _moveDelegates(address(0), delegates[to] == address(0) ? to : delegates[to], amount);
        _writeTotalSupplyCheckpoint();          // S22a: track for quorum
    }

    /// @notice Called by ZbxStaking when ZBX is unstaked — remove governance power.
    function burn(address from, uint256 amount) external onlyStaking {
        _burn(from, amount);
        _moveDelegates(delegates[from] == address(0) ? from : delegates[from], address(0), amount);
        _writeTotalSupplyCheckpoint();          // S22a: track for quorum
    }

    // ─── Delegation ───────────────────────────────────────────────────────

    /// @notice Delegate your votes to another address (or yourself).
    function delegate(address delegatee) external {
        address current = delegates[msg.sender];
        uint256 balance = balanceOf(msg.sender);

        delegates[msg.sender] = delegatee;
        emit DelegateChanged(msg.sender, current, delegatee);

        _moveDelegates(current == address(0) ? msg.sender : current,
                       delegatee == address(0) ? msg.sender : delegatee,
                       balance);
    }

    // ─── Vote queries ─────────────────────────────────────────────────────

    /// @notice Current voting power of an address.
    function getVotes(address account) external view returns (uint256) {
        uint256 nCheckpoints = numCheckpoints[account];
        return nCheckpoints == 0 ? 0 : _checkpoints[account][nCheckpoints - 1].votes;
    }

    /// @notice Historical voting power at a specific block.
    /// @dev    S22a-fix1 (governance integrity): two Compound-style
    ///         boundary guards added BEFORE the binary search. Without
    ///         them, querying a `blockNumber` strictly BEFORE the first
    ///         checkpoint would (incorrectly) fall through the loop and
    ///         return the first checkpoint's votes — letting a voter mint
    ///         AFTER the proposal snapshot and have their post-snapshot
    ///         votes count for that proposal. With the guards in place,
    ///         pre-first-checkpoint queries correctly return 0.
    function getPriorVotes(address account, uint256 blockNumber) external view returns (uint256) {
        require(blockNumber < block.number, "ZBXGov: not yet determined");

        uint256 nCheckpoints = numCheckpoints[account];
        if (nCheckpoints == 0) return 0;

        // S22a-fix1 boundary 1: queried block at-or-after the latest
        // checkpoint → return latest (no need to binary-search).
        if (_checkpoints[account][nCheckpoints - 1].fromBlock <= blockNumber) {
            return _checkpoints[account][nCheckpoints - 1].votes;
        }
        // S22a-fix1 boundary 2: queried block strictly BEFORE the first
        // checkpoint → no votes existed yet → return 0. (Prior to this
        // guard, the binary-search final-return would have wrongly
        // returned the first checkpoint's votes — a governance bypass.)
        if (_checkpoints[account][0].fromBlock > blockNumber) {
            return 0;
        }

        // Binary search for most recent checkpoint at or before blockNumber.
        // Invariant from the boundary checks above:
        //     first.fromBlock <= blockNumber < latest.fromBlock
        uint256 lower = 0;
        uint256 upper = nCheckpoints - 1;
        while (upper > lower) {
            uint256 center = upper - (upper - lower) / 2;
            Checkpoint memory cp = _checkpoints[account][center];
            if (cp.fromBlock == blockNumber) return cp.votes;
            if (cp.fromBlock < blockNumber) { lower = center; } else { upper = center - 1; }
        }
        return _checkpoints[account][lower].votes;
    }

    // ─── Non-transferable override ────────────────────────────────────────

    function _beforeTransfer(address from, address to, uint256) internal pure override {
        // Allow mint (from == 0) and burn (to == 0) but block peer transfers.
        require(from == address(0) || to == address(0), "ZBXGov: non-transferable");
    }

    // ─── Internal checkpoint ──────────────────────────────────────────────

    function _moveDelegates(address src, address dst, uint256 amount) private {
        if (src == dst || amount == 0) return;

        if (src != address(0)) {
            uint256 n = numCheckpoints[src];
            uint256 old = n > 0 ? _checkpoints[src][n - 1].votes : 0;
            uint256 updated = old - amount;
            _writeCheckpoint(src, n, old, updated);
        }

        if (dst != address(0)) {
            uint256 n = numCheckpoints[dst];
            uint256 old = n > 0 ? _checkpoints[dst][n - 1].votes : 0;
            uint256 updated = old + amount;
            _writeCheckpoint(dst, n, old, updated);
        }
    }

    /// @dev S22b (defense-in-depth + audit symmetry): bound `newVotes` to
    ///      uint224 BEFORE the narrowing cast, mirroring the existing
    ///      `_writeTotalSupplyCheckpoint` bound. In the soulbound 1:1 model
    ///      per-account votes ≤ total supply ≤ uint224.max, so any
    ///      hypothetical overflow attempt reverts before state persists —
    ///      EITHER the total-supply guard OR this per-account guard
    ///      independently guarantees the upper ceiling. Call-order detail
    ///      (S22b architect re-review note): `mint()` runs `_moveDelegates`
    ///      BEFORE `_writeTotalSupplyCheckpoint`, so this per-account
    ///      guard may actually fire first in an attack — they are
    ///      belt-and-suspenders, not sequenced. Defends against any
    ///      future delegation refactor that could push aggregate
    ///      per-account delegated weight above the per-account-balance
    ///      ceiling, and ensures the silent narrowing cast
    ///      `uint224(newVotes)` can never truncate. The architect
    ///      (S22a-fix1 re-review) flagged this asymmetry as optional
    ///      hardening; closing it here.
    function _writeCheckpoint(address delegatee, uint256 nCheckpoints, uint256 oldVotes, uint256 newVotes) private {
        require(newVotes <= type(uint224).max, "ZBXGov: votes overflow uint224");
        uint32 blockNumber = uint32(block.number);
        if (nCheckpoints > 0 && _checkpoints[delegatee][nCheckpoints - 1].fromBlock == blockNumber) {
            _checkpoints[delegatee][nCheckpoints - 1].votes = uint224(newVotes);
        } else {
            _checkpoints[delegatee].push(Checkpoint({fromBlock: blockNumber, votes: uint224(newVotes)}));
            numCheckpoints[delegatee] = nCheckpoints + 1;
        }
        emit DelegateVotesChanged(delegatee, oldVotes, newVotes);
    }

    // ─── Snapshots-via-Votes semantics (S22b) ────────────────────────────
    //
    // ZBXGov intentionally does NOT implement an OpenZeppelin-style
    // ERC20Snapshot module with `_snapshot()`/`balanceOfAt`. Instead, the
    // checkpoint history maintained for governance vote weighting (the
    // `_checkpoints[account]` and `_totalSupplyCheckpoints` arrays
    // populated by `_moveDelegates`/`_writeTotalSupplyCheckpoint`) IS the
    // snapshot facility — and it is consumed exclusively by ZbxGovernor
    // through `getPriorVotes(account, blockNumber)` and
    // `totalSupplyAt(blockNumber)`.
    //
    // Why this is sufficient for the soulbound governance model:
    //   1. ZBXGov is non-transferable (see `_beforeTransfer` above), so
    //      "balance at block X" only changes via mint/burn — both of which
    //      already write a per-account checkpoint via `_moveDelegates`.
    //   2. Vote weight = delegated weight, captured at proposal snapshot
    //      block (= startBlock - 1, with votingDelay = 1 block this equals
    //      the propose block). Post-snapshot mints DO NOT count toward the
    //      proposal — see test 14 in GovernorVotesIntegration.t.sol.
    //   3. Quorum is computed against historical total supply via
    //      `totalSupplyAt`, so proposals cannot be diluted or hardened by
    //      post-snapshot supply changes.
    //
    // Out-of-scope for ZBXGov (separately trackable):
    //   * Raw-balance snapshots for non-governance use cases (e.g.
    //     "airdrop based on holders at block X" where the airdrop must
    //     ignore delegation). The current checkpoints reflect delegated
    //     weight, not raw balance — they coincide for accounts that have
    //     never delegated and never received a delegation, but diverge
    //     once `delegate()` is called. A `balanceOfAt(account, block)`
    //     view would need its own checkpoint array.
    //   * ERC-5805 (Voting with Delegation) compatibility — interface
    //     parity with OpenZeppelin's IVotes is intentionally not claimed.
    //     ZbxGovernor consumes the readers directly.
    //
    // ─── Total-supply queries / writer (S22a) ────────────────────────────

    /// @notice Historical total supply at a strictly-past block.
    /// @dev    Same convention as getPriorVotes: `blockNumber` must satisfy
    ///         `blockNumber < block.number`. Used by ZbxGovernor.quorum to
    ///         compute the percentage threshold against the supply at the
    ///         proposal's snapshot block (= startBlock - 1).
    ///
    ///         S22a-fix1: same Compound-style boundary guards as
    ///         getPriorVotes — without them, a pre-first-checkpoint
    ///         query would wrongly return the first checkpoint's value.
    function totalSupplyAt(uint256 blockNumber) external view returns (uint256) {
        require(blockNumber < block.number, "ZBXGov: not yet determined");

        uint256 n = _totalSupplyCheckpoints.length;
        if (n == 0) return 0;

        // S22a-fix1 boundary 1: at-or-after latest → latest.
        if (_totalSupplyCheckpoints[n - 1].fromBlock <= blockNumber) {
            return _totalSupplyCheckpoints[n - 1].votes;
        }
        // S22a-fix1 boundary 2: strictly before first → 0.
        if (_totalSupplyCheckpoints[0].fromBlock > blockNumber) {
            return 0;
        }

        // Binary search for most recent checkpoint at or before blockNumber.
        // Invariant: first.fromBlock <= blockNumber < latest.fromBlock.
        uint256 lower = 0;
        uint256 upper = n - 1;
        while (upper > lower) {
            uint256 center = upper - (upper - lower) / 2;
            Checkpoint memory cp = _totalSupplyCheckpoints[center];
            if (cp.fromBlock == blockNumber) return cp.votes;
            if (cp.fromBlock <  blockNumber) { lower = center; } else { upper = center - 1; }
        }
        return _totalSupplyCheckpoints[lower].votes;
    }

    /// @dev Write a total-supply checkpoint after every mint/burn.
    ///      Bounds the cast: `totalSupply()` must fit in uint224. This
    ///      caps the soulbound supply at type(uint224).max wei
    ///      (= 2^224 - 1 ≈ 2.7×10^67 wei, i.e. ~2.7×10^49 ZBX with the
    ///      18-decimal denomination), many orders of magnitude beyond
    ///      any conceivable issuance schedule. Defense-in-depth: the
    ///      uint224 cast itself would silently wrap on overflow, which
    ///      would let an attacker who somehow overflowed supply make
    ///      ZbxGovernor.quorum trivially small (since it scales 4% of
    ///      this stored value).
    function _writeTotalSupplyCheckpoint() private {
        uint32  blockNumber = uint32(block.number);
        uint256 supply      = totalSupply();
        require(supply <= type(uint224).max, "ZBXGov: supply overflow uint224");

        uint256 n = _totalSupplyCheckpoints.length;
        if (n > 0 && _totalSupplyCheckpoints[n - 1].fromBlock == blockNumber) {
            _totalSupplyCheckpoints[n - 1].votes = uint224(supply);
        } else {
            _totalSupplyCheckpoints.push(Checkpoint({fromBlock: blockNumber, votes: uint224(supply)}));
        }
    }

    // ─── Admin ────────────────────────────────────────────────────────────

    function setStakingContract(address sc) external onlyOwner {
        emit StakingContractUpdated(stakingContract, sc);
        stakingContract = sc;
    }

    function transferOwnership(address to) external onlyOwner {
        emit OwnershipTransferred(owner, to); owner = to;
    }

    function _owner() internal view override returns (address) { return owner; }
}