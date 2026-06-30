// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title  ZbxRaffle — Provably fair on-chain raffle using ZbxVRF
/// @author Zebvix Technologies Pvt Ltd
///
/// @notice Anyone can create a raffle: set ticket price, max tickets, prize
///         distribution, and draw time.  Winners are selected using ZbxVRF
///         so the outcome cannot be manipulated by any party.
///
///         Prize tiers:
///           - 1st prize: 50% of pot
///           - 2nd prize: 30% of pot
///           - 3rd prize: 10% of pot
///           - Protocol fee: 5%
///           - Raffle creator: 5%
///
///         Tickets: native ZBX or any ERC-20.
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:module     Gaming / Raffle (ZEP-039)

interface IZbxVRF {
    function requestRandom(bytes32 seedHash) external returns (bytes32 requestId);
    function fulfillRandom(bytes32 requestId, bytes32 seed) external returns (uint256 randomness);
    function isRevealable(bytes32 requestId) external view returns (bool);
}

interface IERC20Raffle {
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function transfer(address to, uint256 amount) external returns (bool);
}

contract ZbxRaffle {

    // ─── Errors ───────────────────────────────────────────────────────────

    error RaffleNotFound();
    error RaffleNotOpen();
    error RaffleClosed();
    error MaxTicketsReached();
    error NotEnoughParticipants();
    error VRFNotReady();
    error AlreadyDrawn();
    error NotCreator();
    error ZeroAmount();
    error ZeroAddress();
    error WithdrawFailed();
    error NotOwner();

    // ─── Events ───────────────────────────────────────────────────────────

    event RaffleCreated(
        uint256 indexed raffleId,
        address indexed creator,
        address         ticketToken,
        uint256         ticketPrice,
        uint256         maxTickets,
        uint256         drawTime
    );
    event TicketPurchased(uint256 indexed raffleId, address indexed buyer, uint256 ticketId);
    event DrawInitiated(uint256 indexed raffleId, bytes32 vrfRequestId);
    event WinnersDrawn(
        uint256 indexed raffleId,
        address first,
        address second,
        address third,
        uint256 pot
    );
    event RaffleCancelled(uint256 indexed raffleId);
    event Refunded(uint256 indexed raffleId, address indexed buyer, uint256 amount);

    // ─── Constants ────────────────────────────────────────────────────────

    uint256 public constant PRIZE_FIRST  = 5000; // 50%
    uint256 public constant PRIZE_SECOND = 3000; // 30%
    uint256 public constant PRIZE_THIRD  = 1000; // 10%
    uint256 public constant CREATOR_CUT  = 250;  //  2.5%
    uint256 public constant PROTOCOL_CUT = 250;  //  2.5%
    uint256 public constant MIN_DRAW_PARTICIPANTS = 3;

    // ─── Types ────────────────────────────────────────────────────────────

    enum RaffleState { Open, DrawPending, Drawn, Cancelled }

    struct Raffle {
        address     creator;
        address     ticketToken;    // address(0) = native ZBX
        uint256     ticketPrice;
        uint256     maxTickets;
        uint256     drawTime;       // earliest time to initiate draw
        RaffleState state;
        bytes32     vrfRequestId;
        address[]   tickets;        // ticket[i] = owner of ticket i
    }

    // ─── State ────────────────────────────────────────────────────────────

    address public vrfContract;
    address public owner;
    address public treasury;

    mapping(uint256 => Raffle) public raffles;
    uint256 public raffleCount;

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor(address vrfContract_, address treasury_) {
        require(vrfContract_ != address(0) && treasury_ != address(0), "Raffle: zero address");
        vrfContract = vrfContract_;
        treasury    = treasury_;
        owner       = msg.sender;
    }

    // ─── Raffle creation ──────────────────────────────────────────────────

    /// @notice Create a new raffle.
    /// @param  ticketToken   ERC-20 token for tickets, or address(0) for ZBX.
    /// @param  ticketPrice   Price per ticket.
    /// @param  maxTickets    Maximum tickets (0 = unlimited, drawn by time).
    /// @param  drawTime      Earliest timestamp when draw can be initiated.
    function createRaffle(
        address ticketToken,
        uint256 ticketPrice,
        uint256 maxTickets,
        uint256 drawTime
    ) external returns (uint256 raffleId) {
        if (ticketPrice == 0) revert ZeroAmount();
        if (drawTime <= block.timestamp) revert RaffleNotOpen();

        raffleId = ++raffleCount;
        raffles[raffleId] = Raffle({
            creator:      msg.sender,
            ticketToken:  ticketToken,
            ticketPrice:  ticketPrice,
            maxTickets:   maxTickets,
            drawTime:     drawTime,
            state:        RaffleState.Open,
            vrfRequestId: bytes32(0),
            tickets:      new address[](0)
        });

        emit RaffleCreated(raffleId, msg.sender, ticketToken, ticketPrice, maxTickets, drawTime);
    }

    // ─── Buy tickets ──────────────────────────────────────────────────────

    /// @notice Buy one or more tickets.  Each ticket is a separate entry.
    function buyTickets(uint256 raffleId, uint256 count) external payable {
        if (count == 0) revert ZeroAmount();
        Raffle storage r = raffles[raffleId];
        if (r.creator == address(0))  revert RaffleNotFound();
        if (r.state != RaffleState.Open) revert RaffleNotOpen();
        if (block.timestamp >= r.drawTime) revert RaffleClosed();
        if (r.maxTickets > 0 && r.tickets.length + count > r.maxTickets)
            revert MaxTicketsReached();

        uint256 totalCost = r.ticketPrice * count;
        if (r.ticketToken == address(0)) {
            require(msg.value >= totalCost, "Raffle: insufficient ZBX");
            if (msg.value > totalCost) {
                (bool ok,) = msg.sender.call{value: msg.value - totalCost}("");
                require(ok, "Raffle: refund failed");
            }
        } else {
            require(IERC20Raffle(r.ticketToken).transferFrom(msg.sender, address(this), totalCost),
                    "Raffle: transfer failed");
        }

        for (uint256 i; i < count; ++i) {
            r.tickets.push(msg.sender);
            emit TicketPurchased(raffleId, msg.sender, r.tickets.length - 1);
        }
    }

    // ─── Draw — Phase 1: Commit ────────────────────────────────────────────

    /// @notice Initiate the draw.  Caller commits a seed hash to ZbxVRF.
    ///         Anyone can call this once drawTime has passed.
    function initiateDraw(uint256 raffleId, bytes32 seedHash) external {
        Raffle storage r = raffles[raffleId];
        if (r.creator == address(0))       revert RaffleNotFound();
        if (r.state != RaffleState.Open)   revert AlreadyDrawn();
        if (block.timestamp < r.drawTime)  revert RaffleNotOpen();
        if (r.tickets.length < MIN_DRAW_PARTICIPANTS) revert NotEnoughParticipants();

        bytes32 requestId = IZbxVRF(vrfContract).requestRandom(seedHash);
        r.state        = RaffleState.DrawPending;
        r.vrfRequestId = requestId;

        emit DrawInitiated(raffleId, requestId);
    }

    // ─── Draw — Phase 2: Reveal ────────────────────────────────────────────

    /// @notice Reveal the seed and draw winners.  Must be called ≥1 block
    ///         after initiateDraw by the same caller (or anyone with the seed).
    function completeDraw(uint256 raffleId, bytes32 seed) external {
        Raffle storage r = raffles[raffleId];
        if (r.creator == address(0))              revert RaffleNotFound();
        if (r.state != RaffleState.DrawPending)    revert AlreadyDrawn();
        if (!IZbxVRF(vrfContract).isRevealable(r.vrfRequestId)) revert VRFNotReady();

        uint256 randomness = IZbxVRF(vrfContract).fulfillRandom(r.vrfRequestId, seed);
        r.state = RaffleState.Drawn;

        uint256 n = r.tickets.length;
        // SEC-2026-05-09 Pass-15 (HIGH-S03): pre-fix three winners were
        // derived from a single seed via a modulo chain
        // (`randomness % n`, `(randomness / n) % n`, ...). For small n
        // the three slots are deterministically correlated — knowing
        // any one winner reveals the others. Replaced with three
        // independent draws keyed by index.
        uint256 r1 = uint256(keccak256(abi.encode(randomness, uint8(0))));
        uint256 r2 = uint256(keccak256(abi.encode(randomness, uint8(1))));
        uint256 r3 = uint256(keccak256(abi.encode(randomness, uint8(2))));
        address first  = r.tickets[r1 % n];
        address second = r.tickets[r2 % n];
        address third  = r.tickets[r3 % n];

        uint256 pot = r.tickets.length * r.ticketPrice;
        _distribute(r.ticketToken, pot, first, second, third, r.creator);

        emit WinnersDrawn(raffleId, first, second, third, pot);
    }

    // ─── Cancel & refund ──────────────────────────────────────────────────

    /// @notice Creator cancels raffle before draw (refunds all buyers).
    function cancelRaffle(uint256 raffleId) external {
        Raffle storage r = raffles[raffleId];
        if (r.creator != msg.sender) revert NotCreator();
        if (r.state != RaffleState.Open) revert AlreadyDrawn();

        r.state = RaffleState.Cancelled;

        // Refund all buyers
        uint256 price = r.ticketPrice;
        address token = r.ticketToken;
        for (uint256 i; i < r.tickets.length; ++i) {
            if (r.tickets[i] != address(0)) {
                address buyer = r.tickets[i];
                r.tickets[i] = address(0); // prevent double-refund
                _send(token, buyer, price);
                emit Refunded(raffleId, buyer, price);
            }
        }

        emit RaffleCancelled(raffleId);
    }

    // ─── View helpers ─────────────────────────────────────────────────────

    function ticketCount(uint256 raffleId) external view returns (uint256) {
        return raffles[raffleId].tickets.length;
    }

    function ticketOwner(uint256 raffleId, uint256 ticketId) external view returns (address) {
        return raffles[raffleId].tickets[ticketId];
    }

    // ─── Internal helpers ─────────────────────────────────────────────────

    function _distribute(
        address token,
        uint256 pot,
        address first,
        address second,
        address third,
        address creator
    ) private {
        _send(token, first,    (pot * PRIZE_FIRST)  / 10_000);
        _send(token, second,   (pot * PRIZE_SECOND) / 10_000);
        _send(token, third,    (pot * PRIZE_THIRD)  / 10_000);
        _send(token, creator,  (pot * CREATOR_CUT)  / 10_000);
        _send(token, treasury, (pot * PROTOCOL_CUT) / 10_000);
    }

    function _send(address token, address to, uint256 amount) private {
        if (amount == 0 || to == address(0)) return;
        if (token == address(0)) {
            (bool ok,) = to.call{value: amount}("");
            require(ok, "Raffle: ZBX send failed");
        } else {
            require(IERC20Raffle(token).transfer(to, amount), "Raffle: token send failed");
        }
    }

    receive() external payable {}
}
