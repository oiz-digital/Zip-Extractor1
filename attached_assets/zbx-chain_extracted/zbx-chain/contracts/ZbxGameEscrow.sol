// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title  ZbxGameEscrow — Trustless game session escrow
/// @author Zebvix Technologies Pvt Ltd
///
/// @notice Holds player stakes during a game session and releases them to
///         the winner once an authorised game contract resolves the outcome.
///
///         Flow:
///           1. Player A calls createSession(token, stake, gameContract)
///              → transfers stake into escrow, returns sessionId.
///           2. Player B calls joinSession(sessionId)
///              → transfers matching stake into escrow.
///           3. Game contract calls resolveSession(sessionId, winner)
///              → escrow releases winner.stake * 2 (minus protocol fee).
///           4. If B never joins, A can cancelSession after TIMEOUT blocks.
///
///         Supports ERC-20 stakes and native ZBX (use address(0) as token).
///         Protocol fee (default 1 %) is sent to the fee recipient.
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:module     Gaming / Escrow (ZEP-031)

interface IERC20Game {
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function transfer(address to, uint256 amount) external returns (bool);
}

contract ZbxGameEscrow {

    // ─── Errors ───────────────────────────────────────────────────────────

    error SessionNotOpen();
    error SessionNotPending();
    error NotAuthorizedGame();
    error NotSessionCreator();
    error StakeMismatch();
    error InvalidWinner();
    error SessionTimedOut();
    error SessionNotTimedOut();
    error ZeroStake();
    error ZeroGameContract();
    error InvalidFee();
    error InvalidRecipient();

    // ─── Events ───────────────────────────────────────────────────────────

    event SessionCreated(
        bytes32 indexed sessionId,
        address indexed playerA,
        address indexed gameContract,
        address         token,
        uint256         stake
    );
    event SessionJoined(
        bytes32 indexed sessionId,
        address indexed playerB
    );
    event SessionResolved(
        bytes32 indexed sessionId,
        address indexed winner,
        uint256         payout,
        uint256         fee
    );
    event SessionCancelled(
        bytes32 indexed sessionId,
        address indexed playerA,
        string          reason
    );
    event FeeUpdated(uint256 newFeeBps);
    event FeeRecipientUpdated(address newRecipient);

    // ─── Types ────────────────────────────────────────────────────────────

    enum SessionState { Open, Active, Resolved, Cancelled }

    struct Session {
        address       playerA;
        address       playerB;
        address       gameContract;   // only this address can resolve
        address       token;          // ERC-20 token or address(0) for native ZBX
        uint256       stake;          // per-player stake
        uint256       createdBlock;
        SessionState  state;
    }

    // ─── Constants ────────────────────────────────────────────────────────

    /// @notice Blocks before an Open session can be cancelled by playerA.
    uint256 public constant TIMEOUT_BLOCKS = 300; // ~25 minutes at 5s blocks

    /// @notice Maximum protocol fee (5 %).
    uint256 public constant MAX_FEE_BPS = 500;

    // ─── State ────────────────────────────────────────────────────────────

    mapping(bytes32 => Session) public sessions;

    /// @notice Protocol fee in basis points (default 100 = 1 %).
    uint256 public feeBps = 100;

    /// @notice Address that receives protocol fees.
    address public feeRecipient;

    /// @notice Owner (can update fee and recipient).
    address public owner;

    uint256 private _nonce;

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor(address feeRecipient_) {
        require(feeRecipient_ != address(0), "Escrow: zero fee recipient");
        feeRecipient = feeRecipient_;
        owner        = msg.sender;
    }

    // ─── Session lifecycle ────────────────────────────────────────────────

    /// @notice Create a new game session.  Caller is player A.
    /// @param  token        ERC-20 token address, or address(0) for native ZBX.
    /// @param  stake        Amount each player must commit.
    /// @param  gameContract Address of the game contract that will resolve
    ///                      the session.  Must be a deployed contract.
    /// @return sessionId    Unique identifier for the new session.
    function createSession(
        address token,
        uint256 stake,
        address gameContract
    ) external payable returns (bytes32 sessionId) {
        if (stake == 0)               revert ZeroStake();
        if (gameContract == address(0)) revert ZeroGameContract();

        _receiveStake(msg.sender, token, stake);

        sessionId = keccak256(abi.encodePacked(
            msg.sender, token, stake, gameContract, block.number, ++_nonce
        ));

        sessions[sessionId] = Session({
            playerA:      msg.sender,
            playerB:      address(0),
            gameContract: gameContract,
            token:        token,
            stake:        stake,
            createdBlock: block.number,
            state:        SessionState.Open
        });

        emit SessionCreated(sessionId, msg.sender, gameContract, token, stake);
    }

    /// @notice Join an open session as player B.
    ///         Must match the exact stake posted by player A.
    function joinSession(bytes32 sessionId) external payable {
        Session storage s = sessions[sessionId];
        if (s.state != SessionState.Open) revert SessionNotOpen();

        _receiveStake(msg.sender, s.token, s.stake);

        s.playerB = msg.sender;
        s.state   = SessionState.Active;

        emit SessionJoined(sessionId, msg.sender);
    }

    /// @notice Resolve a session.  Only callable by the session's gameContract.
    /// @param  sessionId  The session to resolve.
    /// @param  winner     Must be playerA or playerB.
    function resolveSession(bytes32 sessionId, address winner) external {
        Session storage s = sessions[sessionId];
        if (s.state != SessionState.Active)       revert SessionNotPending();
        if (msg.sender != s.gameContract)          revert NotAuthorizedGame();
        if (winner != s.playerA && winner != s.playerB) revert InvalidWinner();

        s.state = SessionState.Resolved;

        uint256 totalPot = s.stake * 2;
        uint256 fee      = (totalPot * feeBps) / 10_000;
        uint256 payout   = totalPot - fee;

        _sendStake(winner,       s.token, payout);
        if (fee > 0) _sendStake(feeRecipient, s.token, fee);

        emit SessionResolved(sessionId, winner, payout, fee);
    }

    /// @notice Cancel an Open session (before playerB joins).
    ///         PlayerA can cancel any time; anyone can cancel after TIMEOUT_BLOCKS.
    function cancelSession(bytes32 sessionId) external {
        Session storage s = sessions[sessionId];
        if (s.state != SessionState.Open) revert SessionNotOpen();

        bool byCreator  = msg.sender == s.playerA;
        bool timedOut   = block.number >= s.createdBlock + TIMEOUT_BLOCKS;

        if (!byCreator && !timedOut) revert SessionNotTimedOut();

        s.state = SessionState.Cancelled;
        _sendStake(s.playerA, s.token, s.stake);

        string memory reason = timedOut ? "timeout" : "creator";
        emit SessionCancelled(sessionId, s.playerA, reason);
    }

    // ─── Tournament pool (N-player) ───────────────────────────────────────

    /// @notice Distribute a tournament pot to multiple winners with weights.
    ///         Called by an authorised game contract directly — no escrow
    ///         session needed (game contract holds the pot itself and calls
    ///         this as a helper for proportional distribution).
    ///
    /// @param  token    ERC-20 token (address(0) = native ZBX).
    /// @param  winners  Ordered list of winner addresses.
    /// @param  shares   Proportional shares (must sum to 10_000 bps).
    /// @param  totalPot Total amount to distribute (must be pre-approved or pre-held).
    function distributeTournament(
        address         token,
        address[] calldata winners,
        uint256[] calldata shares,
        uint256         totalPot
    ) external {
        require(winners.length == shares.length, "Escrow: length mismatch");
        require(winners.length > 0,              "Escrow: empty winners");

        uint256 sumShares;
        for (uint256 i; i < shares.length; ++i) sumShares += shares[i];
        require(sumShares == 10_000, "Escrow: shares must sum to 10000");

        uint256 fee    = (totalPot * feeBps) / 10_000;
        uint256 pot    = totalPot - fee;

        for (uint256 i; i < winners.length; ++i) {
            require(winners[i] != address(0), "Escrow: zero winner");
            uint256 amount = (pot * shares[i]) / 10_000;
            _sendStake(winners[i], token, amount);
        }
        if (fee > 0) _sendStake(feeRecipient, token, fee);
    }

    // ─── Admin ────────────────────────────────────────────────────────────

    function setFeeBps(uint256 bps) external {
        require(msg.sender == owner, "Escrow: not owner");
        if (bps > MAX_FEE_BPS) revert InvalidFee();
        feeBps = bps;
        emit FeeUpdated(bps);
    }

    function setFeeRecipient(address r) external {
        require(msg.sender == owner, "Escrow: not owner");
        if (r == address(0)) revert InvalidRecipient();
        feeRecipient = r;
        emit FeeRecipientUpdated(r);
    }

    // ─── Internal stake helpers ───────────────────────────────────────────

    function _receiveStake(address from, address token, uint256 amount) private {
        if (token == address(0)) {
            require(msg.value == amount, "Escrow: wrong ZBX amount");
        } else {
            require(IERC20Game(token).transferFrom(from, address(this), amount),
                    "Escrow: transfer failed");
        }
    }

    function _sendStake(address to, address token, uint256 amount) private {
        if (token == address(0)) {
            (bool ok,) = to.call{value: amount}("");
            require(ok, "Escrow: ZBX send failed");
        } else {
            require(IERC20Game(token).transfer(to, amount),
                    "Escrow: token send failed");
        }
    }

    receive() external payable {}
}
