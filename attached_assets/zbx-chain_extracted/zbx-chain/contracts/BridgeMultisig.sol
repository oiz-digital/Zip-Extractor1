// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import { IBridgeVault } from "./interfaces/IBridgeVault.sol";

/// @title BridgeMultisig — N-of-M oracle multisig for the Zebvix → BSC mint
/// @author Zebvix Technologies Pvt Ltd
/// @notice Each relayer in the oracle set independently watches Zebvix
///         `BridgeOutEvent`s and submits an EIP-191 personal-sign signature
///         to this contract via `submitMint`. Once `threshold` distinct
///         signatures arrive for the same `(zebvixSeq, to, amount)` tuple
///         the contract calls `BridgeVault.executeMint`, which then mints
///         wrapped ZBX to the user via the token's vault-only mint path.
///
/// @dev    Phase B.12 deploys a 1-of-1 single-key oracle (founder.key).
///         Phase B.13 upgrades to 5-of-7 with independent relayer custody.
///
///         Vault-deadlock fix (post-architect-review): vault is mutable +
///         set-once via `setVault(address)` then permanently locked. This
///         lets the deploy script create Multisig first, then Vault (which
///         needs the multisig address), then call `setVault` + `lockVault`
///         on both the multisig and the token.
contract BridgeMultisig {

    // ─── S25-Y4 unchecked policy ─────────────────────────────────────────
    // All `unchecked { ... }` blocks in this contract fall into ONE of these
    // proven-safe categories (per S25 hardening pass):
    //   (a) post-require subtraction — preceding `require(x >= y)` proves
    //       `x - y` cannot underflow (token balance debits, allowance debits).
    //   (b) conservation pair — incrementing one slot by exactly the value
    //       just decremented (or vice-versa) from another, with the totalSupply
    //       invariant pre-checked (mint/burn/transfer leg of accounting pair).
    //   (c) bounded for-loop counter — `for (i; i < len; ) { ...; unchecked
    //       { i++; } }` where `len` is the bound; standard gas-saving pattern.
    //   (d) modular wrap intentional — uint32 timestamp/sequence wrap arithmetic
    //       (Uniswap V2 style); the wrap IS the spec.
    //   (e) UQ112x112 fixed-point shift — pre-bounded by uint112 reserve
    //       invariants (Uniswap V2 oracle accumulator).
    // Reviewers MUST classify any future `unchecked` block in this file
    // against one of (a)-(e) before merging; new categories require AUDIT entry.
    // ─────────────────────────────────────────────────────────────────────
    // ---------------------------------------------------------------------
    // Constants
    // ---------------------------------------------------------------------

    /// @notice Domain tag mixed into the personal-sign digest so a relayer
    ///         signature is bound to *this* bridge / chain / vault tuple
    ///         and cannot be replayed across deployments.
    /// @dev    Audit-2026-05-01 S6-BM1 (CRITICAL/Defense-in-Depth): version
    ///         bumped to v2. The digest now also binds the SOURCE chain id
    ///         (Zebvix L1 chain id, immutable per deployment) — without it,
    ///         a relayer signature for `(zebvixSeq=42, to, amount)` produced
    ///         on Zebvix testnet was byte-identical to one for Zebvix mainnet
    ///         if the BSC bridge contract was reused across deployments.
    ///         Binding `sourceChainId` makes that misuse impossible at the
    ///         contract level, not just by deployment discipline. Old v1
    ///         signatures will no longer verify — relayers must re-sign.
    bytes32 private constant _DOMAIN_TAG = keccak256("ZEBVIX_BRIDGE_MINT_v2");

    /// @notice Source chain id for the Zebvix L1 network this bridge serves
    ///         (e.g. 8989 for mainnet). Set once at construction. Bound into
    ///         every relayer signature digest to prevent cross-source replay.
    uint64 public immutable sourceChainId;

    // ---------------------------------------------------------------------
    // Wiring
    // ---------------------------------------------------------------------

    /// @notice BridgeVault address (set once via setVault, then locked).
    address public vault;
    bool    public vaultLocked;

    /// @notice Quorum size. Immutable so an owner can't reduce it post-deploy.
    uint256 public immutable threshold; // M

    // ---------------------------------------------------------------------
    // Mutable state
    // ---------------------------------------------------------------------

    address public founder;
    /// @notice 2-step founder-transfer destination. The successor must
    ///         explicitly call `acceptFounder()` to take over — single-step
    ///         transfer was removed because mistyping the new address would
    ///         brick the multisig and freeze the bridge.
    address public pendingFounder;
    /// @notice High-risk admin executor (typically `ZbxTimelock`). When zero,
    ///         the founder may still call `onlyAdmin` functions for bootstrap;
    ///         once set, those revert unless `msg.sender == governor`.
    address public governor;
    bool    public paused;

    /// @notice Active relayer set. Founder can rotate via `setRelayers`.
    address[] private _relayers;
    mapping(address => bool) public isRelayer;

    /// @notice Quarantine: a relayer that has just been removed cannot be
    ///         re-added within `RELAYER_QUARANTINE` seconds. This blocks the
    ///         "compromise founder → remove honest relayer → re-add attacker
    ///         key disguised as the original" attack: the attacker would
    ///         have to wait a full day, giving the team time to detect the
    ///         compromise and rotate the founder key.
    mapping(address => uint40) public removedAt;
    uint256 public constant RELAYER_QUARANTINE = 1 days;
    error RelayerQuarantined(address relayer, uint40 untilTimestamp);

    /// @notice Per-zebvixSeq, per-relayer voting record. Prevents the same
    ///         relayer signing twice for the same seq.
    mapping(uint64 => mapping(address => bool)) public votedBy;

    /// @notice Per-zebvixSeq vote count + canonical (to, amount) the
    ///         relayers must agree on. First valid signature defines them;
    ///         later signatures must match exactly or are rejected.
    struct Tally {
        uint64  count;
        address to;
        uint256 amount;
        bool    executed;
    }
    mapping(uint64 => Tally) public tallies;

    // ---------------------------------------------------------------------
    // Errors
    // ---------------------------------------------------------------------

    error NotFounder();
    error NotPendingFounder();
    error NotAdmin();
    error PausedErr();
    error NotRelayer(address who);
    error AlreadyVoted(uint64 seq, address who);
    error AlreadyExecuted(uint64 seq);
    error MismatchedTally(uint64 seq, address to, uint256 amount);
    error InvalidSignature();
    error EmptyRelayers();
    error ThresholdAboveSet();
    error VaultNotSet();
    error VaultAlreadyLocked();
    error ZeroAddress();

    // ---------------------------------------------------------------------
    // Events
    // ---------------------------------------------------------------------

    event Voted(uint64 indexed zebvixSeq, address indexed relayer, uint64 count);
    event Executed(uint64 indexed zebvixSeq, address indexed to, uint256 amount);
    event RelayersUpdated(address[] relayers);
    event FounderTransferStarted(address indexed currentFounder, address indexed pendingFounder);
    event FounderTransferred(address indexed from, address indexed to);
    event GovernorSet(address indexed previousGovernor, address indexed newGovernor);
    event PausedSet(bool isPaused);
    event VaultSet(address indexed vault);
    event VaultLocked();

    // ---------------------------------------------------------------------
    // Constructor
    // ---------------------------------------------------------------------

    constructor(
        address[] memory initialRelayers,
        uint256 _threshold,
        address _founder,
        uint64 _sourceChainId
    ) {
        if (_founder == address(0)) revert ZeroAddress();
        if (initialRelayers.length == 0) revert EmptyRelayers();
        if (_threshold == 0 || _threshold > initialRelayers.length) revert ThresholdAboveSet();
        if (_sourceChainId == 0) revert ZeroAddress(); // reuse error: 0 is never a valid chain id

        threshold     = _threshold;
        founder       = _founder;
        sourceChainId = _sourceChainId;
        _setRelayers(initialRelayers);
    }

    // ---------------------------------------------------------------------
    // Modifiers
    // ---------------------------------------------------------------------

    modifier onlyFounder() {
        if (msg.sender != founder) revert NotFounder();
        _;
    }

    /// @notice High-risk admin gate. Routes to the timelock once
    ///         `setGovernor(timelock)` is wired; otherwise falls back to
    ///         the founder. After governance is enabled, direct founder
    ///         calls to `onlyAdmin` functions revert — a stolen founder
    ///         key alone cannot rotate the relayer set or pause the bridge
    ///         indefinitely.
    modifier onlyAdmin() {
        if (governor != address(0)) {
            if (msg.sender != governor) revert NotAdmin();
        } else {
            if (msg.sender != founder) revert NotAdmin();
        }
        _;
    }

    modifier whenNotPaused() {
        if (paused) revert PausedErr();
        _;
    }

    modifier vaultReady() {
        if (vault == address(0)) revert VaultNotSet();
        _;
    }

    // ---------------------------------------------------------------------
    // Vault wiring (set + lock)
    // ---------------------------------------------------------------------

    function setVault(address _vault) external onlyFounder {
        if (vaultLocked) revert VaultAlreadyLocked();
        if (_vault == address(0)) revert ZeroAddress();
        vault = _vault;
        emit VaultSet(_vault);
    }

    function lockVault() external onlyFounder {
        if (vault == address(0)) revert VaultNotSet();
        if (vaultLocked) revert VaultAlreadyLocked();
        vaultLocked = true;
        emit VaultLocked();
    }

    // ---------------------------------------------------------------------
    // Submit a mint vote
    // ---------------------------------------------------------------------

    /// @notice A relayer (or anyone, on behalf of a relayer) submits an
    ///         EIP-191 sig. Once `threshold` distinct relayer sigs arrive
    ///         for `(seq, to, amount)`, the vault is invoked and the user
    ///         receives wrapped ZBX via the token's vault-only mint path.
    /// @param  zebvixSeq Sequence number from Zebvix `BridgeOutEvent`.
    /// @param  to        Recipient on BSC.
    /// @param  amount    Amount in 18-decimal wei.
    /// @param  v,r,s     EIP-191 personal_sign signature components.
    function submitMint(
        uint64 zebvixSeq,
        address to,
        uint256 amount,
        uint8 v,
        bytes32 r,
        bytes32 s
    ) public whenNotPaused vaultReady {
        // 1. Recover signer.
        // Audit-2026-05-01 S6-BM1: `sourceChainId` (Zebvix L1 chain id) is
        // now bound into the digest alongside `block.chainid` (BSC). This
        // closes the cross-source replay vector where a signature collected
        // for Zebvix testnet → BSC was reusable for Zebvix mainnet → BSC
        // when the same BSC bridge was misdeployed against both sources.
        bytes32 inner = keccak256(
            abi.encode(_DOMAIN_TAG, block.chainid, sourceChainId, vault, zebvixSeq, to, amount)
        );
        // EIP-191 personal_sign prefix: byte 0x19 + literal text + 32 (length).
        // Earlier versions had double-escaped backslashes here ("\\x19...\\n32"),
        // which Solidity stored as the literal 5-char string `\x19` instead of
        // the single 0x19 byte — causing every relayer signature to fail
        // ecrecover. Use bytes1(0x19) and the actual newline so the digest
        // matches `eth_sign` / `personal_sign` output from standard wallets.
        bytes32 digest = keccak256(
            abi.encodePacked(bytes1(0x19), "Ethereum Signed Message:\n32", inner)
        );
        address signer = ecrecover(digest, v, r, s);
        if (signer == address(0)) revert InvalidSignature();
        if (!isRelayer[signer]) revert NotRelayer(signer);

        // 2. Replay/dup checks.
        Tally storage t = tallies[zebvixSeq];
        if (t.executed) revert AlreadyExecuted(zebvixSeq);
        if (votedBy[zebvixSeq][signer]) revert AlreadyVoted(zebvixSeq, signer);

        // 3. Lock canonical (to, amount) on first vote; reject mismatches.
        if (t.count == 0) {
            t.to = to;
            t.amount = amount;
        } else if (t.to != to || t.amount != amount) {
            revert MismatchedTally(zebvixSeq, to, amount);
        }

        votedBy[zebvixSeq][signer] = true;
        unchecked {
            t.count += 1;
        }

        emit Voted(zebvixSeq, signer, t.count);

        // 4. If quorum reached, execute mint via vault. Vault is the sole
        //    minter on the token; it passes `zebvixSeq` explicitly to the
        //    token's `bridgeMint(to, amount, seq)`. No transient storage.
        if (t.count >= threshold) {
            t.executed = true;
            IBridgeVault(vault).executeMint(to, amount, zebvixSeq);
            emit Executed(zebvixSeq, to, amount);
        }
    }

    /// @notice MS1 griefing mitigation — admin can reset a fraudulent or
    ///         corrupted tally for a given `zebvixSeq` BEFORE it executes.
    ///
    /// @dev    Without this, a single compromised relayer who votes first with
    ///         garbage `(to, amount)` locks the tally permanently: all honest
    ///         relayers get `MismatchedTally` and the user's funds are bricked.
    ///         `cancelTally` lets the admin (timelock / founder pre-governance)
    ///         wipe the tally so legitimate relayers can re-vote with the
    ///         correct `(to, amount)`. Gated by `onlyAdmin` so a stolen founder
    ///         key cannot cancel an already-executed tally — `AlreadyExecuted`
    ///         is checked first.
    ///
    ///         Note: `votedBy[seq][addr]` is NOT reset — relayers who already
    ///         voted (including the griefing one) cannot vote again after the
    ///         cancel. The operator MUST rotate out the compromised relayer via
    ///         `setRelayers` and wait `RELAYER_QUARANTINE` before the new key
    ///         can participate. This is intentional: it forces key rotation on
    ///         any detected compromise rather than allowing silent re-use.
    function cancelTally(uint64 zebvixSeq) external onlyAdmin {
        Tally storage t = tallies[zebvixSeq];
        if (t.executed) revert AlreadyExecuted(zebvixSeq);
        // Reset vote count and canonical fields — leaves votedBy intact so
        // existing voters cannot re-vote under the new canonical (to, amount).
        t.count  = 0;
        t.to     = address(0);
        t.amount = 0;
    }

    /// @notice Variant that accepts a batch of pre-collected sigs in one tx.
    ///         Internal call (not `this.`) so a single relayer's bad sig
    ///         doesn't cost the others gas. Bails on the first reverting
    ///         sig — caller should prune duplicates / post-quorum sigs.
    function submitMintBatch(
        uint64 zebvixSeq,
        address to,
        uint256 amount,
        uint8[] calldata vs,
        bytes32[] calldata rs,
        bytes32[] calldata ss
    ) external whenNotPaused vaultReady {
        require(vs.length == rs.length && rs.length == ss.length, "LEN_MISMATCH");
        for (uint256 i = 0; i < vs.length; i++) {
            // Stop early if quorum already reached this batch — saves gas
            // and avoids the post-execute revert path.
            if (tallies[zebvixSeq].executed) break;
            submitMint(zebvixSeq, to, amount, vs[i], rs[i], ss[i]);
        }
    }

    // ---------------------------------------------------------------------
    // Founder ops
    // ---------------------------------------------------------------------

    /// @notice Emergency pause/unpause. Stays `onlyFounder` (NOT onlyAdmin)
    ///         because pausing is a defensive action that may need to happen
    ///         within seconds — going through a 48h timelock would defeat the
    ///         purpose. The asymmetric risk is acceptable: a stolen founder
    ///         key can DoS the bridge but cannot drain it or rotate relayers.
    function setPaused(bool _p) external onlyFounder {
        paused = _p;
        emit PausedSet(_p);
    }

    /// @notice Step 1 of 2-step founder transfer. Single-step transfer was
    ///         removed because mistyping `newFounder` would brick the
    ///         multisig and freeze the bridge permanently.
    function transferFounder(address newFounder) external onlyFounder {
        pendingFounder = newFounder;
        emit FounderTransferStarted(founder, newFounder);
    }

    /// @notice Step 2 of 2-step founder transfer.
    function acceptFounder() external {
        if (msg.sender != pendingFounder) revert NotPendingFounder();
        emit FounderTransferred(founder, pendingFounder);
        founder        = pendingFounder;
        pendingFounder = address(0);
    }

    /// @notice Hand high-risk admin control to a timelock / governance
    ///         executor. Gated by `onlyAdmin` so the cutover is one-way
    ///         from the founder's perspective:
    ///
    ///         - **Pre-cutover** (`governor == 0`): founder bootstraps the
    ///           timelock address.
    ///         - **Post-cutover** (`governor != 0`): only the current
    ///           governor (the timelock) may change `governor`. A stolen
    ///           founder key cannot simply re-point the governor to an
    ///           attacker EOA and then immediately call `setRelayers` to
    ///           install attacker-controlled validators — every governor
    ///           change must itself wait the full timelock delay.
    function setGovernor(address newGovernor) external onlyAdmin {
        emit GovernorSet(governor, newGovernor);
        governor = newGovernor;
    }

    /// @notice Rotate the relayer set. This is THE most security-critical
    ///         admin function in the bridge — whoever can call it can
    ///         install attacker-controlled relayers and mint arbitrary
    ///         wrapped ZBX. Therefore gated by `onlyAdmin` (timelock once
    ///         governance is enabled). The per-relayer quarantine
    ///         (`RELAYER_QUARANTINE = 1 days`) is enforced inside
    ///         `_setRelayers` and remains in force regardless of caller.
    function setRelayers(address[] calldata newSet) external onlyAdmin {
        if (newSet.length == 0) revert EmptyRelayers();
        if (threshold > newSet.length) revert ThresholdAboveSet();
        _setRelayers(newSet);
    }

    // ---------------------------------------------------------------------
    // Read helpers
    // ---------------------------------------------------------------------

    function relayers() external view returns (address[] memory) {
        return _relayers;
    }

    function relayerCount() external view returns (uint256) {
        return _relayers.length;
    }

    // ---------------------------------------------------------------------
    // Internal
    // ---------------------------------------------------------------------

    function _setRelayers(address[] memory newSet) internal {
        // Mark which addresses are in the new set (for quarantine bypass below).
        // Use a small in-memory bitset of new-set membership to avoid O(N*M).
        // For typical N,M ≤ 20 a nested loop is cheaper than allocating.

        // Stamp removed-at for each old relayer that is NOT in the new set.
        for (uint256 i = 0; i < _relayers.length; i++) {
            address oldR = _relayers[i];
            isRelayer[oldR] = false;
            bool stillIn = false;
            for (uint256 j = 0; j < newSet.length; j++) {
                if (newSet[j] == oldR) { stillIn = true; break; }
            }
            if (!stillIn) {
                removedAt[oldR] = uint40(block.timestamp);
            }
        }
        delete _relayers;

        // Install new set, dedup-checked + quarantine-checked.
        for (uint256 i = 0; i < newSet.length; i++) {
            address r = newSet[i];
            require(r != address(0), "ZERO_RELAYER");
            require(!isRelayer[r], "DUP_RELAYER");

            uint40 lastRemoved = removedAt[r];
            if (lastRemoved != 0
                && block.timestamp < uint256(lastRemoved) + RELAYER_QUARANTINE)
            {
                revert RelayerQuarantined(
                    r, uint40(uint256(lastRemoved) + RELAYER_QUARANTINE)
                );
            }

            isRelayer[r] = true;
            _relayers.push(r);
        }

        emit RelayersUpdated(newSet);
    }
}