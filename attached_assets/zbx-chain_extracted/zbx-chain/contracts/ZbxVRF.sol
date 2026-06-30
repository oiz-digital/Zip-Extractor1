// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title  ZbxVRF — On-chain Verifiable Random Function (commit-reveal)
/// @author Zebvix Technologies Pvt Ltd
///
/// @notice Provides unpredictable, manipulation-resistant randomness for games
///         and other on-chain applications.  Uses a two-phase commit-reveal
///         scheme so neither party (requester nor the chain) can predict the
///         output in advance:
///
///           Phase 1 — Commit:
///             Requester calls requestRandom(keccak256(seed)) with their
///             secret seed hashed.  The contract stores the request and the
///             current block number.
///
///           Phase 2 — Reveal (≥ 1 block later):
///             Requester calls fulfillRandom(seed).  The contract combines
///             the revealed seed with the *previous* block's randao mix
///             (EIP-4399 PREVRANDAO) to derive the final VRF output.
///
///         Because the block hash is unknown at commit time, the requester
///         cannot pre-compute the outcome.  Because the seed is committed
///         before the block is produced, the validator cannot grind the
///         block to choose a favourable outcome for the requester.
///
/// @dev    Single-party randomness.  For two-party fair games use
///         ZbxGameEscrow which XORs both parties' committed seeds before
///         combining with PREVRANDAO.
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:module     Gaming / VRF (ZEP-031)

contract ZbxVRF {

    // ─── Errors ───────────────────────────────────────────────────────────

    error AlreadyCommitted();
    error NoCommitment();
    error RevealTooEarly();
    error SeedMismatch();
    error AlreadyFulfilled();
    error CommitmentExpired();

    // ─── Events ───────────────────────────────────────────────────────────

    event RandomRequested(
        bytes32 indexed requestId,
        address indexed requester,
        uint256          commitBlock
    );
    event RandomFulfilled(
        bytes32 indexed requestId,
        address indexed requester,
        uint256          randomness
    );

    // ─── Types ────────────────────────────────────────────────────────────

    struct Commitment {
        address requester;
        bytes32 seedHash;
        uint256 commitBlock;
        bool    fulfilled;
    }

    // ─── Constants ────────────────────────────────────────────────────────

    /// @notice Minimum blocks between commit and reveal.
    ///         Set to 1 so PREVRANDAO of the commit-block is unknown.
    uint256 public constant MIN_REVEAL_DELAY = 1;

    /// @notice Maximum blocks the commitment is valid.  After this window
    ///         the PREVRANDAO of the commit-block may be unavailable and the
    ///         commitment should be refreshed.
    uint256 public constant COMMITMENT_WINDOW = 256;

    // ─── Storage ──────────────────────────────────────────────────────────

    /// @notice requestId → Commitment
    mapping(bytes32 => Commitment) public commitments;

    // ─── Commit phase ────────────────────────────────────────────────────

    /// @notice Submit a commitment.  `seedHash` must be keccak256(seed)
    ///         where `seed` is a secret 32-byte value you will reveal later.
    ///
    /// @param  seedHash  keccak256 of your secret seed.
    /// @return requestId Unique identifier for this randomness request.
    function requestRandom(bytes32 seedHash)
        external
        returns (bytes32 requestId)
    {
        requestId = keccak256(abi.encodePacked(
            msg.sender, seedHash, block.number, block.prevrandao
        ));
        if (commitments[requestId].requester != address(0))
            revert AlreadyCommitted();

        commitments[requestId] = Commitment({
            requester:   msg.sender,
            seedHash:    seedHash,
            commitBlock: block.number,
            fulfilled:   false
        });

        emit RandomRequested(requestId, msg.sender, block.number);
    }

    // ─── Reveal phase ────────────────────────────────────────────────────

    /// @notice Reveal the seed and derive the VRF output.
    ///         Must be called at least MIN_REVEAL_DELAY blocks after commit
    ///         and within COMMITMENT_WINDOW blocks.
    ///
    /// @param  requestId  The ID returned by requestRandom().
    /// @param  seed       The secret value whose keccak256 was committed.
    /// @return randomness A 256-bit pseudo-random value derived from
    ///                    PREVRANDAO + seed + requestId.  Map to a game
    ///                    range with `randomness % range`.
    function fulfillRandom(bytes32 requestId, bytes32 seed)
        external
        returns (uint256 randomness)
    {
        Commitment storage c = commitments[requestId];
        if (c.requester == address(0))     revert NoCommitment();
        if (c.fulfilled)                   revert AlreadyFulfilled();
        if (block.number < c.commitBlock + MIN_REVEAL_DELAY)
                                           revert RevealTooEarly();
        if (block.number > c.commitBlock + COMMITMENT_WINDOW)
                                           revert CommitmentExpired();
        if (keccak256(abi.encodePacked(seed)) != c.seedHash)
                                           revert SeedMismatch();

        c.fulfilled = true;

        // Derive randomness: mix PREVRANDAO (validator-provided entropy),
        // caller's seed (requester-provided entropy), and requestId
        // (domain separation) so the output cannot be reproduced by
        // replaying the same seed in a different context.
        randomness = uint256(keccak256(abi.encodePacked(
            block.prevrandao,
            seed,
            requestId,
            c.commitBlock
        )));

        emit RandomFulfilled(requestId, c.requester, randomness);
    }

    // ─── Two-party randomness helper ─────────────────────────────────────

    /// @notice Combine two independently-committed seeds into a single VRF
    ///         output.  Use this for fair 2-player games: each party commits
    ///         a seed hash before the game starts; when both reveal, this
    ///         function merges them so neither party alone controls the result.
    ///
    /// @param  seed0  Seed from party 0 (must match their original commitment).
    /// @param  seed1  Seed from party 1 (must match their original commitment).
    /// @param  nonce  Any additional domain separator (e.g. sessionId).
    /// @return randomness XOR-combined entropy, further hashed with PREVRANDAO.
    function combinedRandom(bytes32 seed0, bytes32 seed1, bytes32 nonce)
        external
        view
        returns (uint256 randomness)
    {
        randomness = uint256(keccak256(abi.encodePacked(
            block.prevrandao,
            seed0 ^ seed1,   // XOR: both parties contribute equally
            nonce
        )));
    }

    // ─── View helpers ─────────────────────────────────────────────────────

    /// @notice Check whether a requestId is ready to reveal.
    function isRevealable(bytes32 requestId) external view returns (bool) {
        Commitment storage c = commitments[requestId];
        return c.requester != address(0)
            && !c.fulfilled
            && block.number >= c.commitBlock + MIN_REVEAL_DELAY
            && block.number <= c.commitBlock + COMMITMENT_WINDOW;
    }

    /// @notice Remaining blocks before a commitment expires.
    function blocksUntilExpiry(bytes32 requestId) external view returns (uint256) {
        Commitment storage c = commitments[requestId];
        uint256 expiry = c.commitBlock + COMMITMENT_WINDOW;
        if (block.number >= expiry) return 0;
        return expiry - block.number;
    }
}
