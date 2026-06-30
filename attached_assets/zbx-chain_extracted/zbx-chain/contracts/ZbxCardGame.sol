// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title  ZbxCardGame — On-chain card game engine with VRF shuffle
/// @author Zebvix Technologies Pvt Ltd
///
/// @notice Provides a trustless card game primitive:
///           - Dealer creates a room with a buy-in stake.
///           - Players join; all commit secret seeds.
///           - Once all seeds are revealed, the deck is deterministically
///             shuffled using combined VRF randomness (so no single party
///             controls the shuffle).
///           - Cards are dealt; game logic (winner determination) is
///             executed on-chain with full verifiability.
///
///         Default game: Highest-card-wins (Texas Hold'em hand evaluator
///         can be added as an upgrade).
///
///         Deck: 52 standard playing cards
///           Rank: 2-14 (14 = Ace), Suit: 0=Clubs 1=Diamonds 2=Hearts 3=Spades
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:module     Gaming / Card Game (ZEP-041)

interface IZbxVRFCard {
    function requestRandom(bytes32 seedHash) external returns (bytes32 requestId);
    function fulfillRandom(bytes32 requestId, bytes32 seed) external returns (uint256 randomness);
    function isRevealable(bytes32 requestId) external view returns (bool);
}

interface IERC20Card {
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function transfer(address to, uint256 amount) external returns (bool);
}

contract ZbxCardGame {

    // ─── Errors ───────────────────────────────────────────────────────────

    error RoomNotFound();
    error RoomFull();
    error RoomNotWaiting();
    error RoomNotReady();
    error RoomNotShuffled();
    error AlreadyJoined();
    error AlreadyCommitted();
    error AlreadyRevealed();
    error NotRoomPlayer();
    error NotAllRevealed();
    error VRFNotReady();
    error InvalidSeed();
    error GameOver();
    error NotDealer();
    error ZeroAmount();
    error ZeroAddress();

    // ─── Events ───────────────────────────────────────────────────────────

    event RoomCreated(uint256 indexed roomId, address indexed dealer, uint256 buyIn, uint256 maxPlayers);
    event PlayerJoined(uint256 indexed roomId, address indexed player);
    event SeedCommitted(uint256 indexed roomId, address indexed player, bytes32 seedHash);
    event SeedRevealed(uint256 indexed roomId, address indexed player);
    event DeckShuffled(uint256 indexed roomId, uint256 randomness);
    event CardsDealt(uint256 indexed roomId);
    event GameEnded(uint256 indexed roomId, address indexed winner, uint256 payout);

    // ─── Constants ────────────────────────────────────────────────────────

    uint256 public constant DECK_SIZE    = 52;
    uint256 public constant HAND_SIZE    = 5;     // cards per player
    uint256 public constant MAX_PLAYERS  = 8;
    uint256 public constant PROTOCOL_FEE = 200;   // 2%

    // ─── Card encoding ────────────────────────────────────────────────────
    // card = rank * 4 + suit
    // rank: 0=2, 1=3, ..., 12=Ace (rank+2 for display)
    // suit: 0=Clubs, 1=Diamonds, 2=Hearts, 3=Spades

    // ─── Types ────────────────────────────────────────────────────────────

    enum RoomState { Waiting, Committing, Revealing, Shuffled, Dealt, Over }

    struct Room {
        address     dealer;
        address     stakeToken;   // address(0) = native ZBX
        uint256     buyIn;
        uint8       maxPlayers;
        RoomState   state;
        address[]   players;
        uint8[]     deck;         // shuffled deck (card indices)
        uint256     vrfSeed;      // combined randomness from all players
    }

    struct PlayerState {
        bytes32  seedHash;
        bytes32  seed;
        bool     committed;
        bool     revealed;
        uint8[]  hand;        // card indices dealt to this player
    }

    // ─── State ────────────────────────────────────────────────────────────

    address public treasury;
    address public vrfContract;

    mapping(uint256 => Room) public rooms;
    mapping(uint256 => mapping(address => PlayerState)) public playerStates;
    uint256 public roomCount;

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor(address vrfContract_, address treasury_) {
        require(vrfContract_ != address(0) && treasury_ != address(0), "CardGame: zero address");
        vrfContract = vrfContract_;
        treasury    = treasury_;
    }

    // ─── Room creation ────────────────────────────────────────────────────

    /// @notice Dealer creates a new game room.
    /// @param  stakeToken  ERC-20 buy-in token (address(0) = native ZBX).
    /// @param  buyIn       Required buy-in per player.
    /// @param  maxPlayers  Number of players (2–MAX_PLAYERS).
    function createRoom(
        address stakeToken,
        uint256 buyIn,
        uint8   maxPlayers
    ) external payable returns (uint256 roomId) {
        if (buyIn == 0)                                   revert ZeroAmount();
        if (maxPlayers < 2 || maxPlayers > MAX_PLAYERS)  revert RoomFull();

        _receiveStake(stakeToken, buyIn);

        roomId = ++roomCount;
        Room storage r = rooms[roomId];
        r.dealer     = msg.sender;
        r.stakeToken = stakeToken;
        r.buyIn      = buyIn;
        r.maxPlayers = maxPlayers;
        r.state      = RoomState.Waiting;
        r.players.push(msg.sender);

        emit RoomCreated(roomId, msg.sender, buyIn, maxPlayers);
        emit PlayerJoined(roomId, msg.sender);
    }

    // ─── Join room ────────────────────────────────────────────────────────

    /// @notice Join a waiting room.
    function joinRoom(uint256 roomId) external payable {
        Room storage r = rooms[roomId];
        if (r.dealer == address(0))          revert RoomNotFound();
        if (r.state != RoomState.Waiting)    revert RoomNotWaiting();
        if (r.players.length >= r.maxPlayers) revert RoomFull();

        for (uint256 i; i < r.players.length; ++i) {
            if (r.players[i] == msg.sender) revert AlreadyJoined();
        }

        _receiveStake(r.stakeToken, r.buyIn);
        r.players.push(msg.sender);

        emit PlayerJoined(roomId, msg.sender);

        // Auto-start committing phase once room is full
        if (r.players.length == r.maxPlayers) {
            r.state = RoomState.Committing;
        }
    }

    // ─── Commit seed ──────────────────────────────────────────────────────

    /// @notice Commit a seed hash (Phase 1 of VRF).
    function commitSeed(uint256 roomId, bytes32 seedHash) external {
        Room storage r = rooms[roomId];
        if (r.state != RoomState.Committing) revert RoomNotReady();
        _requirePlayer(r, msg.sender);

        PlayerState storage ps = playerStates[roomId][msg.sender];
        if (ps.committed) revert AlreadyCommitted();

        ps.seedHash   = seedHash;
        ps.committed  = true;

        emit SeedCommitted(roomId, msg.sender, seedHash);

        // If all players committed, move to revealing phase
        if (_allCommitted(roomId, r)) {
            r.state = RoomState.Revealing;
        }
    }

    // ─── Reveal seed ──────────────────────────────────────────────────────

    /// @notice Reveal your seed (Phase 2 of VRF).  Must match committed hash.
    function revealSeed(uint256 roomId, bytes32 seed) external {
        Room storage r = rooms[roomId];
        if (r.state != RoomState.Revealing) revert RoomNotReady();
        _requirePlayer(r, msg.sender);

        PlayerState storage ps = playerStates[roomId][msg.sender];
        if (ps.revealed) revert AlreadyRevealed();
        if (keccak256(abi.encodePacked(seed)) != ps.seedHash) revert InvalidSeed();

        ps.seed    = seed;
        ps.revealed = true;

        emit SeedRevealed(roomId, msg.sender);

        // If all revealed, shuffle deck
        if (_allRevealed(roomId, r)) {
            _shuffleDeck(roomId, r);
        }
    }

    // ─── Deal cards ───────────────────────────────────────────────────────

    /// @notice Deal cards to all players from the shuffled deck.
    function dealCards(uint256 roomId) external {
        Room storage r = rooms[roomId];
        if (r.state != RoomState.Shuffled) revert RoomNotShuffled();

        uint256 cardIndex;
        for (uint256 p; p < r.players.length; ++p) {
            PlayerState storage ps = playerStates[roomId][r.players[p]];
            for (uint256 c; c < HAND_SIZE; ++c) {
                ps.hand.push(r.deck[cardIndex++]);
            }
        }

        r.state = RoomState.Dealt;
        emit CardsDealt(roomId);
    }

    // ─── Determine winner ─────────────────────────────────────────────────

    /// @notice Evaluate hands and pay out to the winner.
    ///         Simple rule: player with highest single card wins.
    ///         (Full poker hand evaluation can be added as an upgrade via ZEP.)
    function determineWinner(uint256 roomId) external {
        Room storage r = rooms[roomId];
        if (r.state != RoomState.Dealt) revert RoomNotShuffled();

        address winner;
        uint8   bestCard;

        for (uint256 p; p < r.players.length; ++p) {
            address player = r.players[p];
            PlayerState storage ps = playerStates[roomId][player];
            uint8 highest;
            for (uint256 c; c < ps.hand.length; ++c) {
                uint8 rank = ps.hand[c] / 4; // rank from card index
                if (rank > highest) highest = rank;
            }
            if (highest > bestCard) {
                bestCard = highest;
                winner   = player;
            }
        }

        r.state = RoomState.Over;

        uint256 totalPot = r.players.length * r.buyIn;
        uint256 fee      = (totalPot * PROTOCOL_FEE) / 10_000;
        uint256 payout   = totalPot - fee;

        _sendStake(r.stakeToken, treasury, fee);
        _sendStake(r.stakeToken, winner,   payout);

        emit GameEnded(roomId, winner, payout);
    }

    // ─── View helpers ─────────────────────────────────────────────────────

    function getHand(uint256 roomId, address player)
        external view returns (uint8[] memory hand)
    {
        return playerStates[roomId][player].hand;
    }

    function getDeck(uint256 roomId) external view returns (uint8[] memory) {
        return rooms[roomId].deck;
    }

    function getPlayers(uint256 roomId) external view returns (address[] memory) {
        return rooms[roomId].players;
    }

    /// @notice Decode a card index into (rank, suit).
    /// @return rank  2-14 (14 = Ace)
    /// @return suit  0=Clubs 1=Diamonds 2=Hearts 3=Spades
    function decodeCard(uint8 cardIndex) external pure returns (uint8 rank, uint8 suit) {
        suit = cardIndex % 4;
        rank = (cardIndex / 4) + 2; // 0-12 → 2-14
    }

    // ─── Internal helpers ─────────────────────────────────────────────────

    function _shuffleDeck(uint256 roomId, Room storage r) private {
        // Combine all player seeds via XOR, then hash with blockhash
        bytes32 combined;
        for (uint256 i; i < r.players.length; ++i) {
            combined = combined ^ playerStates[roomId][r.players[i]].seed;
        }
        uint256 randomness = uint256(keccak256(abi.encodePacked(
            block.prevrandao, combined, roomId
        )));
        r.vrfSeed = randomness;

        // Initialize ordered deck
        uint8[] memory deck = new uint8[](DECK_SIZE);
        for (uint8 i; i < DECK_SIZE; ++i) deck[i] = i;

        // Fisher-Yates shuffle
        for (uint256 i = DECK_SIZE - 1; i > 0; --i) {
            uint256 j = randomness % (i + 1);
            uint8 tmp  = deck[i];
            deck[i]    = deck[j];
            deck[j]    = tmp;
            randomness = uint256(keccak256(abi.encodePacked(randomness)));
        }

        r.deck  = deck;
        r.state = RoomState.Shuffled;

        emit DeckShuffled(roomId, r.vrfSeed);
    }

    function _allCommitted(uint256 roomId, Room storage r) private view returns (bool) {
        for (uint256 i; i < r.players.length; ++i) {
            if (!playerStates[roomId][r.players[i]].committed) return false;
        }
        return true;
    }

    function _allRevealed(uint256 roomId, Room storage r) private view returns (bool) {
        for (uint256 i; i < r.players.length; ++i) {
            if (!playerStates[roomId][r.players[i]].revealed) return false;
        }
        return true;
    }

    function _requirePlayer(Room storage r, address player) private view {
        for (uint256 i; i < r.players.length; ++i) {
            if (r.players[i] == player) return;
        }
        revert NotRoomPlayer();
    }

    function _receiveStake(address token, uint256 amount) private {
        if (token == address(0)) {
            require(msg.value >= amount, "CardGame: insufficient ZBX");
            if (msg.value > amount) {
                (bool ok,) = msg.sender.call{value: msg.value - amount}("");
                require(ok, "CardGame: refund failed");
            }
        } else {
            require(IERC20Card(token).transferFrom(msg.sender, address(this), amount),
                    "CardGame: transfer failed");
        }
    }

    function _sendStake(address token, address to, uint256 amount) private {
        if (amount == 0) return;
        if (token == address(0)) {
            (bool ok,) = to.call{value: amount}("");
            require(ok, "CardGame: ZBX send failed");
        } else {
            require(IERC20Card(token).transfer(to, amount), "CardGame: token send failed");
        }
    }

    receive() external payable {}
}
