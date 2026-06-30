// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title ZbxBridge — Cross-chain asset bridge.
/// @notice Lock assets on Ethereum/BSC → mint wrapped assets on ZBX Chain.
///         Burn wrapped assets on ZBX Chain → release on source chain.
///
/// @dev   Architecture:
///          Source chain: ZbxBridge (this contract) — lock/release
///          ZBX Chain:    ZbxBridgeMint             — mint/burn
///          Relay:        Trusted relay network with threshold signatures
///
///        Supported assets (Phase 1):
///          ETH  → ZbxETH
///          BTC  → ZbxBTC
///          USDT → ZbxUSDT
///          USDC → ZbxUSDC
///          BNB  → ZbxBNB
///
/// @custom:zbx-chain  Chain ID 8989

import "./libraries/Governable.sol";
import "./libraries/ReentrancyGuard.sol";
import { SafeERC20, IERC20Minimal } from "./libraries/SafeERC20.sol";

// Local alias kept for ABI / event readability; SafeERC20 takes IERC20Minimal.
interface IERC20_Bridge {
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function transfer(address to, uint256 amount) external returns (bool);
}

/// @dev SEC-2026-05-09 hardening pass:
///        - inherits ReentrancyGuard (closes audit finding HIGH-01)
///        - adds Pausable circuit breaker (closes audit finding NICE-16)
///        - enforces minimum threshold floor of 2 (closes audit finding NICE-15)
contract ZbxBridge is Governable, ReentrancyGuard {
    using SafeERC20 for IERC20Minimal;


    // ─── Pausable (SEC-2026-05-09) ────────────────────────────────────────
    //
    // Minimal inline circuit breaker. `pause()` is restricted to onlyAdmin
    // (timelock once governance is live) and freezes all user-facing flows
    // (bridgeOut, bridgeIn) without affecting admin / emergencyWithdraw.
    // Read-only functions remain callable while paused.

    bool public paused;
    event Paused(address indexed by);
    event Unpaused(address indexed by);
    modifier whenNotPaused() {
        require(!paused, "Bridge: paused");
        _;
    }
    function pause()   external onlyAdmin { paused = true;  emit Paused(msg.sender); }
    function unpause() external onlyAdmin { paused = false; emit Unpaused(msg.sender); }

    /// SEC-2026-05-09: minimum signing threshold floor.
    /// A threshold of 1 is structurally indistinguishable from "single
    /// trusted relayer" and defeats the entire multi-sig design — any one
    /// compromised relayer key can release locked funds. We enforce ≥2 in
    /// the constructor and in setThreshold to keep the bridge honestly
    /// multi-sig at all times. The 32-relayer cap is unchanged.
    uint256 internal constant MIN_THRESHOLD = 2;

    // ─── State ────────────────────────────────────────────────────────────

    // `owner` and the 2-step `pendingOwner` transfer pattern come from the
    // Governable base. The bootstrap owner is set in the constructor; once
    // `setGovernor(timelock)` is called, every `onlyAdmin` function below
    // can only be invoked by the timelock — direct owner calls revert.

    address public relayAdmin;

    /// NEW-LOW-01 fix (2026-05-05) — dedicated emergency withdrawal recipient.
    ///
    /// Previously `emergencyWithdraw` sent funds to `owner`.  Once the
    /// protocol hands ownership to the timelock (`setGovernor` is called),
    /// `owner` == the timelock contract.  A malicious governance proposal
    /// could then drain the bridge in the same block it executes — the
    /// timelock GRACE_PERIOD provides no protection because the transfer
    /// is direct (no second-step release).
    ///
    /// Fix: introduce a separate `guardian` cold-wallet address.  The
    /// guardian can only receive funds from `emergencyWithdraw`; it has
    /// no other privileges.  Defaults to `address(0)` (i.e. the bridge
    /// reverts on emergency withdraw until an operator sets it).
    address public guardian;

    /// token → is whitelisted for bridging
    mapping(address => bool) public supportedTokens;

    /// Minimum number of relay signatures required.
    uint256 public threshold;

    /// Nonce: prevents replay attacks on the bridgeIn path.
    mapping(bytes32 => bool) public processedNonces;

    // ─── S36-bridge-out-nonce (S11-BRIDGE-SOL-OUT1 closure) ───────────────
    //
    // Per-sender monotonic counter used to derive a collision-free
    // bridgeOut nonce. Pre-S36 the nonce was
    //     keccak256(msg.sender, token, amount, block.timestamp, block.number)
    // which collides whenever the same sender bridges the same amount of
    // the same token twice in the same block (3-second BSC blocks make
    // this trivial — wallet UIs commonly retry, MEV bots batch). On
    // collision, processedNonces[nonce] gets set on the FIRST bridgeIn
    // and the SECOND user's funds are permanently locked (the relay sees
    // a duplicate nonce and either drops the second event or marks it
    // already-processed). Net effect: silent fund loss with no on-chain
    // error path for the affected user.
    //
    // S36 derives the nonce from
    //     keccak256(block.chainid, address(this), msg.sender, outNonces[msg.sender]++)
    // — collision-free by construction (sender-local counter is
    // strictly monotone) AND replay-resistant across deployments
    // (chainid + contract address bind the nonce to this exact
    // ZbxBridge instance).
    //
    // Backward compatibility: nonce stays bytes32, event signature
    // unchanged, processedNonces[] semantics unchanged. The Rust relay
    // continues to consume `BridgeOutInitiated(token, from, amount,
    // targetChainId, nonce)` verbatim — only the BIT-PATTERN of the
    // nonce differs. No relay-side migration required.
    mapping(address => uint256) public outNonces;

    /// token → total locked amount
    mapping(address => uint256) public lockedAmount;

    /// Maximum single-bridge amount per token (slippage protection).
    mapping(address => uint256) public maxBridgeAmount;

    /// SEC-2026-05-09 Pass-19 (Tier-2 #9): PER-SOURCE-CHAIN hourly
    /// volume rate limit on `bridgeIn` releases. The new `bridgeIn`
    /// signature carries `srcChainId` explicitly (ABI break,
    /// documented), bound into the relayer digest so a compromised
    /// relayer set on chain A cannot replay messages from chain B.
    /// Per-(srcChainId, token) windowing bounds drain to
    /// `bridgeInHourlyLimit[srcChainId][token]` per rolling hour,
    /// giving operators time to detect a single-chain compromise
    /// and `pause()` without freezing flow from healthy chains.
    /// Default cap = 0 means "unset, no limit"; operators MUST set
    /// limits per (srcChainId, token) pair after upgrade — fail-open
    /// is a documented operator footgun.
    uint256 internal constant BRIDGE_IN_WINDOW = 1 hours;
    mapping(uint256 => mapping(address => uint256)) public bridgeInHourlyLimit;
    mapping(uint256 => mapping(address => uint256)) public bridgeInWindowStart;
    mapping(uint256 => mapping(address => uint256)) public bridgeInWindowVolume;
    event BridgeInHourlyLimitUpdated(
        uint256 indexed srcChainId, address indexed token, uint256 newLimit
    );

    /// Authorised relayer signers — only signatures from these addresses
    /// count toward the threshold. Without this set, the previous `bridgeIn`
    /// merely counted the *length* of the supplied signature array and never
    /// recovered any signers, meaning anyone could pass `threshold` empty
    /// bytes arrays. See AUDIT_2026-04-30.md C-17.
    mapping(address => bool) public isRelayer;
    address[] public relayerList;

    /// secp256k1 curve order n / 2 — for low-S enforcement (EIP-2).
    uint256 internal constant HALF_N =
        0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0;

    // ─── Events ───────────────────────────────────────────────────────────

    event BridgeOutInitiated(
        address indexed token, address indexed from,
        uint256 amount, uint256 targetChainId, bytes32 nonce
    );
    event BridgeInCompleted(
        address indexed token, address indexed to,
        uint256 amount, bytes32 nonce
    );
    event TokenWhitelisted(address indexed token, bool status);
    event RelayAdminUpdated(address indexed newAdmin);
    event GuardianUpdated(address indexed newGuardian);

    // ─── Constructor ──────────────────────────────────────────────────────

    /// @param relayAdmin_  Address that can manage relay signers and thresholds.
    /// @param threshold_   Minimum relay signatures required to finalise a bridgeIn.
    /// @param guardian_    Cold-wallet address that receives funds on emergencyWithdraw.
    ///                     Must be a non-custodial cold wallet — NOT the deployer EOA or
    ///                     the timelock.  Set this before going live.
    constructor(address relayAdmin_, uint256 threshold_, address guardian_) Governable(msg.sender) {
        // Enforce same bound as `setThreshold` so a misconfigured deployment
        // cannot deadlock `bridgeIn` (the unique-signer bitmap holds 32 slots).
        require(relayAdmin_ != address(0), "Bridge: zero relayAdmin");
        // SEC-2026-05-09: enforce ≥2 to preserve true multi-sig safety.
        require(threshold_ >= MIN_THRESHOLD && threshold_ <= 32, "Bridge: threshold out of range");
        // OPERATOR-02: guardian must be provided at deploy time so that
        // emergencyWithdraw is immediately operational without a separate
        // setGuardian() call that could be forgotten.
        require(guardian_ != address(0), "Bridge: zero guardian");
        relayAdmin = relayAdmin_;
        threshold  = threshold_;
        guardian   = guardian_;
        emit GuardianUpdated(guardian_);
    }

    // ─── Bridge out (Ethereum → ZBX Chain) ───────────────────────────────

    /// @notice Lock tokens on Ethereum to receive wrapped tokens on ZBX Chain.
    /// @param token        ERC-20 token address (must be whitelisted)
    /// @param amount       Amount to bridge
    /// @param targetAddress Recipient address on ZBX Chain
    function bridgeOut(address token, uint256 amount, bytes memory targetAddress)
        external
        nonReentrant
        whenNotPaused
    {
        require(supportedTokens[token], "Bridge: token not supported");
        require(amount > 0,             "Bridge: zero amount");
        require(amount <= maxBridgeAmount[token], "Bridge: exceeds max");
        // Suppress "unused parameter" warning while preserving the public
        // ABI: targetAddress is consumed off-chain by the relay (which
        // re-reads it from the call's calldata via tx-trace). The S11-
        // BRIDGE-SOL-OUT2 finding (no source-chain binding of
        // targetAddress in the event) is a separate audit item out of
        // scope for the S36 nonce-collision closure.
        targetAddress;

        // S36-bridge-out-nonce (S11-BRIDGE-SOL-OUT1 closure):
        // Composite-key nonce derivation. See the outNonces declaration
        // above for the full pre/post-S36 collision discussion.
        //
        // Post-increment: each caller gets a strictly-monotone sequence
        // 0, 1, 2, ... so two bridgeOut calls from the same sender in
        // the same block CANNOT share an outNonces value. Combined with
        // the (chainid, address(this), msg.sender) binding, the
        // resulting bytes32 nonce is unique across:
        //   * intra-block repeats by the same sender (counter advances)
        //   * cross-sender same-block (sender included)
        //   * cross-deployment / cross-chain replay
        //     (chainid + address(this) included — same protection
        //      ZbxBridge.bridgeIn already uses for its digest at
        //      `address(this), block.chainid, token, to, amount, nonce`)
        uint256 senderNonce = outNonces[msg.sender]++;
        bytes32 nonce = keccak256(abi.encode(
            block.chainid, address(this), msg.sender, senderNonce
        ));

        // SEC-2026-05-09: SafeERC20 — handles USDT / BNB style non-bool tokens.
        IERC20Minimal(token).safeTransferFrom(msg.sender, address(this), amount);
        lockedAmount[token] += amount;

        // 8989 = ZBX mainnet chain ID (matches `crates/zbx-types/src/lib.rs::CHAIN_ID`).
        // This event is consumed by the relay; if you change the constant
        // here you MUST coordinate with the Rust relay code in zbx-bridge.
        emit BridgeOutInitiated(token, msg.sender, amount, 8989, nonce);
        // Relay monitors this event and mints on ZBX Chain.
    }

    // ─── Bridge in (ZBX Chain → Ethereum) ────────────────────────────────

    /// @notice Release locked tokens when relay confirms burn on ZBX Chain.
    ///         Requires `threshold` distinct ECDSA signatures from authorised
    ///         relayers over the canonical message
    ///           keccak256(abi.encode(address(this), block.chainid, token, to, amount, nonce))
    ///         wrapped with the EIP-191 personal_sign prefix.
    ///
    ///         The previous version only counted `relaySignatures.length`
    ///         without ever calling `ecrecover` — anyone could call this with
    ///         `threshold` zero-length arrays. See AUDIT_2026-04-30.md C-17.
    function bridgeIn(
        uint256 srcChainId,
        address token, address to, uint256 amount,
        bytes32 nonce, bytes[] calldata relaySignatures
    ) external nonReentrant whenNotPaused {
        // SEC-2026-05-09 Pass-19 (Tier-2 #9): srcChainId now bound
        // into both the digest AND the rate-limit window so a relay
        // compromise on chain A cannot drain volume budget allocated
        // to chain B. Source chains MUST be non-zero; the canonical
        // EVM convention reserves 0 for "unspecified".
        require(srcChainId != 0, "Bridge: zero srcChainId");
        // SOL-02 (LOW): prevent tokens being permanently locked by bridging
        // to the zero address — would burn the recipient's tokens silently.
        require(to != address(0), "Bridge: zero recipient");
        require(!processedNonces[nonce], "Bridge: nonce already used");
        require(relaySignatures.length >= threshold, "Bridge: insufficient relay sigs");
        require(lockedAmount[token] >= amount, "Bridge: insufficient locked balance");

        // Canonical, replay-resistant digest. Includes contract address +
        // chain id (this dest) + srcChainId (origin) so a sig collected
        // for one (src, dest, bridge) tuple cannot be replayed elsewhere.
        bytes32 digest = keccak256(abi.encode(
            address(this), block.chainid, srcChainId, token, to, amount, nonce
        ));
        bytes32 ethDigest = keccak256(abi.encodePacked(
            bytes1(0x19), "Ethereum Signed Message:\n32", digest
        ));

        // Track unique signers via a packed bitmap-on-stack: at most 32
        // distinct relayers per call (limit `threshold` ≤ 32 enforced in
        // setThreshold below).
        uint256 seen;
        uint256 valid;
        for (uint256 i = 0; i < relaySignatures.length; i++) {
            address signer = _recoverWithLowS(ethDigest, relaySignatures[i]);
            if (signer == address(0))            continue;
            if (!isRelayer[signer])              continue;
            uint256 idx = _relayerIndex(signer); // 0..31
            uint256 bit = uint256(1) << idx;
            if (seen & bit != 0)                 continue;   // duplicate
            seen  |= bit;
            valid += 1;
            if (valid >= threshold) break;
        }
        require(valid >= threshold, "Bridge: signature threshold not met");

        // Pass-19 Tier-2 #9: per-(srcChainId, token) hourly cap.
        uint256 cap = bridgeInHourlyLimit[srcChainId][token];
        if (cap > 0) {
            if (block.timestamp - bridgeInWindowStart[srcChainId][token] >= BRIDGE_IN_WINDOW) {
                bridgeInWindowStart[srcChainId][token]  = block.timestamp;
                bridgeInWindowVolume[srcChainId][token] = 0;
            }
            require(
                bridgeInWindowVolume[srcChainId][token] + amount <= cap,
                "Bridge: hourly rate limit"
            );
            bridgeInWindowVolume[srcChainId][token] += amount;
        }

        processedNonces[nonce] = true;
        lockedAmount[token]   -= amount;

        // SEC-2026-05-09: SafeERC20 — non-bool-returning tokens (USDT) supported.
        IERC20Minimal(token).safeTransfer(to, amount);
        emit BridgeInCompleted(token, to, amount, nonce);
    }

    function addRelayer(address relayer) external onlyAdmin {
        require(!isRelayer[relayer], "Bridge: already relayer");
        require(relayerList.length < 32, "Bridge: too many relayers (cap 32)");
        isRelayer[relayer] = true;
        relayerList.push(relayer);
    }

    function removeRelayer(address relayer) external onlyAdmin {
        require(isRelayer[relayer], "Bridge: not a relayer");
        isRelayer[relayer] = false;
        // Compact the list (preserve order is not required).
        for (uint256 i = 0; i < relayerList.length; i++) {
            if (relayerList[i] == relayer) {
                relayerList[i] = relayerList[relayerList.length - 1];
                relayerList.pop();
                break;
            }
        }
    }

    function _relayerIndex(address signer) internal view returns (uint256) {
        for (uint256 i = 0; i < relayerList.length; i++) {
            if (relayerList[i] == signer) return i;
        }
        return type(uint256).max; // never reached for an authorised signer
    }

    /// EIP-2 low-S enforcement + ecrecover. Returns address(0) on any
    /// malformed input (rather than reverting) so caller can simply skip.
    function _recoverWithLowS(bytes32 hash, bytes calldata sig)
        internal pure returns (address)
    {
        if (sig.length != 65) return address(0);
        bytes32 r; bytes32 s; uint8 v;
        // S25-Y3 assembly: split fixed-65-byte ECDSA signature into (r,s,v)
        // via direct calldata reads. Bounds proven by `sig.length != 65 → return`
        // on the previous line. EIP-2 low-S enforcement happens immediately below.
        assembly {
            r := calldataload(sig.offset)
            s := calldataload(add(sig.offset, 32))
            v := byte(0, calldataload(add(sig.offset, 64)))
        }
        if (uint256(s) > HALF_N) return address(0);   // reject high-S (malleable)
        if (v != 27 && v != 28)  return address(0);
        return ecrecover(hash, v, r, s);
    }

    // ─── Admin ────────────────────────────────────────────────────────────

    function whitelistToken(address token, uint256 maxAmount) external onlyAdmin {
        supportedTokens[token]  = true;
        maxBridgeAmount[token]  = maxAmount;
        emit TokenWhitelisted(token, true);
    }

    /// SEC-2026-05-09 Pass-19 (Tier-2 #9): operator-configurable
    /// per-(srcChainId, token) hourly cap on `bridgeIn` releases.
    /// Set to 0 to disable (no cap). Recommended values per pair:
    /// 5–10× the expected legitimate hourly volume so honest flow
    /// is never throttled, attacker drain is bounded.
    function setBridgeInHourlyLimit(
        uint256 srcChainId, address token, uint256 newLimit
    ) external onlyAdmin {
        require(srcChainId != 0, "Bridge: zero srcChainId");
        bridgeInHourlyLimit[srcChainId][token] = newLimit;
        emit BridgeInHourlyLimitUpdated(srcChainId, token, newLimit);
    }

    function setThreshold(uint256 threshold_) external onlyAdmin {
        // Cap matches the seen-bitmap width in `bridgeIn`.
        // SEC-2026-05-09: floor of MIN_THRESHOLD (2) preserves multi-sig safety.
        require(threshold_ >= MIN_THRESHOLD && threshold_ <= 32, "Bridge: threshold out of range");
        threshold = threshold_;
    }

    function setRelayAdmin(address relayAdmin_) external onlyAdmin {
        relayAdmin = relayAdmin_;
        emit RelayAdminUpdated(relayAdmin_);
    }

    /// @notice Set the guardian address that receives funds on emergency withdrawal.
    ///
    /// @dev    NEW-LOW-01 fix (2026-05-05).  The guardian MUST be set to a
    ///         cold-wallet address before `emergencyWithdraw` can be called.
    ///         Only callable by `onlyAdmin` (timelock once governance is live).
    function setGuardian(address guardian_) external onlyAdmin {
        require(guardian_ != address(0), "Bridge: guardian cannot be zero");
        guardian = guardian_;
        emit GuardianUpdated(guardian_);
    }

    /// @notice Emergency withdraw in case of critical bug.
    ///
    /// @dev    NEW-LOW-01 fix (2026-05-05): funds now go to the designated
    ///         `guardian` cold-wallet, NOT to `owner`.
    ///
    ///         Previously this sent to `owner`, which after `setGovernor` is
    ///         called becomes the timelock.  A malicious governance proposal
    ///         could therefore drain the bridge by encoding an
    ///         `emergencyWithdraw` call — the timelock GRACE_PERIOD provides
    ///         no protection because the ERC-20 transfer is direct with no
    ///         second-step release gate.
    ///
    ///         Gated by `onlyAdmin` (timelock) — a stolen operator key cannot
    ///         call this once governance is live.  Reverts if `guardian` has
    ///         not been set so funds are never sent to `address(0)`.
    function emergencyWithdraw(address token, uint256 amount) external onlyAdmin nonReentrant {
        require(guardian != address(0), "Bridge: guardian not set");
        // SEC-2026-05-09: SafeERC20 ensures USDT / BNB rescue works.
        IERC20Minimal(token).safeTransfer(guardian, amount);
    }
}