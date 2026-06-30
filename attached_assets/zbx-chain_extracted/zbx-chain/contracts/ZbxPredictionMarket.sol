// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title  ZbxPredictionMarket — On-chain prediction / betting market
/// @author Zebvix Technologies Pvt Ltd
///
/// @notice Anyone creates a market with a YES/NO question and a resolution
///         deadline.  An appointed resolver (oracle address or multisig)
///         resolves it to YES, NO, or VOID (no result / draw).
///
///         Bettors stake ERC-20 tokens on their chosen outcome.  Winners
///         share the total pot proportionally to their bet size.
///
///         Use cases:
///           - Sports results ("Will ZBX reach $1 before 2027?")
///           - Crypto price events ("ETH > $10k by Q4?")
///           - Protocol governance ("Will proposal #42 pass?")
///           - Esports, elections, etc.
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:module     Gaming / Prediction Market (ZEP-040)

interface IERC20Pred {
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function transfer(address to, uint256 amount) external returns (bool);
}

contract ZbxPredictionMarket {

    // ─── Errors ───────────────────────────────────────────────────────────

    error MarketNotFound();
    error MarketNotOpen();
    error MarketNotResolved();
    error MarketAlreadyResolved();
    error NotResolver();
    error NotCreator();
    error NothingToClaim();
    error AlreadyClaimed();
    error ZeroAmount();
    error ZeroAddress();
    error InvalidOutcome();
    error DeadlineNotReached();
    error DeadlinePassed();
    error NotAdmin();

    // ─── Events ───────────────────────────────────────────────────────────

    event MarketCreated(
        uint256 indexed marketId,
        string  question,
        address indexed resolver,
        address         betToken,
        uint256         deadline
    );
    event BetPlaced(uint256 indexed marketId, address indexed bettor, bool isYes, uint256 amount);
    event MarketResolved(uint256 indexed marketId, Outcome outcome);
    event Claimed(uint256 indexed marketId, address indexed bettor, uint256 amount);
    event MarketVoided(uint256 indexed marketId);

    // ─── Types ────────────────────────────────────────────────────────────

    enum Outcome { Unresolved, Yes, No, Void }

    struct Market {
        string  question;
        address creator;
        address resolver;     // address that can resolve the market
        address betToken;     // ERC-20 token for bets (address(0) = native ZBX)
        uint256 deadline;     // no new bets after this; resolver can resolve after
        uint256 yesPool;      // total ZBX/token bet on YES
        uint256 noPool;       // total ZBX/token bet on NO
        Outcome outcome;
        bool    creatorFeeWithdrawn;
    }

    struct BetRecord {
        uint256 yesBet;
        uint256 noBet;
        bool    claimed;
    }

    // ─── Constants ────────────────────────────────────────────────────────

    uint256 public constant PROTOCOL_FEE_BPS = 200;  // 2%
    uint256 public constant CREATOR_FEE_BPS  = 100;  // 1%

    // ─── State ────────────────────────────────────────────────────────────

    address public admin;
    address public treasury;

    mapping(uint256 => Market)  public markets;
    uint256 public marketCount;

    mapping(uint256 => mapping(address => BetRecord)) public bets;

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor(address treasury_) {
        require(treasury_ != address(0), "PredMarket: zero treasury");
        admin    = msg.sender;
        treasury = treasury_;
    }

    // ─── Market creation ──────────────────────────────────────────────────

    /// @notice Create a new prediction market.
    /// @param  question  Human-readable question (e.g., "Will ZBX > $1 by 2027?").
    /// @param  resolver  Address authorised to resolve the market.
    /// @param  betToken  ERC-20 token for bets, or address(0) for native ZBX.
    /// @param  deadline  Unix timestamp — bets close and resolver can act after this.
    function createMarket(
        string  calldata question,
        address          resolver,
        address          betToken,
        uint256          deadline
    ) external returns (uint256 marketId) {
        if (resolver == address(0))       revert ZeroAddress();
        if (deadline <= block.timestamp)  revert MarketNotOpen();

        marketId = ++marketCount;
        markets[marketId] = Market({
            question:             question,
            creator:              msg.sender,
            resolver:             resolver,
            betToken:             betToken,
            deadline:             deadline,
            yesPool:              0,
            noPool:               0,
            outcome:              Outcome.Unresolved,
            creatorFeeWithdrawn:  false
        });

        emit MarketCreated(marketId, question, resolver, betToken, deadline);
    }

    // ─── Betting ──────────────────────────────────────────────────────────

    /// @notice Place a bet on YES or NO.
    /// @param  marketId  The market to bet on.
    /// @param  isYes     True = bet on YES, False = bet on NO.
    /// @param  amount    Amount to stake (must be > 0).
    function bet(uint256 marketId, bool isYes, uint256 amount) external payable {
        if (amount == 0) revert ZeroAmount();
        Market storage m = markets[marketId];
        if (m.resolver == address(0))       revert MarketNotFound();
        if (m.outcome != Outcome.Unresolved) revert MarketAlreadyResolved();
        if (block.timestamp >= m.deadline)   revert DeadlinePassed();

        _receiveToken(m.betToken, amount);

        BetRecord storage b = bets[marketId][msg.sender];
        if (isYes) {
            m.yesPool += amount;
            b.yesBet  += amount;
        } else {
            m.noPool  += amount;
            b.noBet   += amount;
        }

        emit BetPlaced(marketId, msg.sender, isYes, amount);
    }

    // ─── Resolution ───────────────────────────────────────────────────────

    /// @notice Resolver calls this to settle the market.
    /// @param  marketId  The market to resolve.
    /// @param  outcome   Yes, No, or Void.
    function resolve(uint256 marketId, Outcome outcome) external {
        Market storage m = markets[marketId];
        if (m.resolver == address(0))        revert MarketNotFound();
        if (msg.sender != m.resolver)        revert NotResolver();
        if (m.outcome != Outcome.Unresolved) revert MarketAlreadyResolved();
        if (block.timestamp < m.deadline)    revert DeadlineNotReached();
        if (outcome == Outcome.Unresolved)   revert InvalidOutcome();

        m.outcome = outcome;
        emit MarketResolved(marketId, outcome);
        if (outcome == Outcome.Void) emit MarketVoided(marketId);
    }

    // ─── Claim winnings ───────────────────────────────────────────────────

    /// @notice Claim winnings (or refund if VOID) after resolution.
    function claim(uint256 marketId) external {
        Market storage m = markets[marketId];
        if (m.resolver == address(0))        revert MarketNotFound();
        if (m.outcome == Outcome.Unresolved) revert MarketNotResolved();

        BetRecord storage b = bets[marketId][msg.sender];
        if (b.claimed) revert AlreadyClaimed();
        b.claimed = true;

        uint256 totalPot = m.yesPool + m.noPool;
        uint256 payout;

        if (m.outcome == Outcome.Void) {
            // Refund original bet
            payout = b.yesBet + b.noBet;
        } else {
            uint256 winPool  = m.outcome == Outcome.Yes ? m.yesPool : m.noPool;
            uint256 userBet  = m.outcome == Outcome.Yes ? b.yesBet  : b.noBet;
            if (userBet == 0) revert NothingToClaim();

            // SEC-2026-05-09 Pass-15 (HIGH-S05 / Pass-12 Tier-2
            // Prediction-double-claim): the share-of-pot must be
            // computed against the LOSING pool only — winners receive
            // their original stake back PLUS a proportional share of
            // the losing side. Pre-fix used `netPot * userBet / winPool`
            // which double-counted the winner's own stake into the
            // distributable pot, so two-sided bettors who happened to
            // be on the winning side claimed both their losing stake
            // (correctly forfeited) and their winning stake's share of
            // their own contribution (incorrectly credited).
            uint256 losePool = m.outcome == Outcome.Yes ? m.noPool : m.yesPool;
            uint256 feeBps   = PROTOCOL_FEE_BPS + CREATOR_FEE_BPS;
            uint256 netLose  = losePool - (losePool * feeBps) / 10_000;
            payout = userBet + (netLose * userBet) / winPool;

            // Take fees once (only when first claimer processes, or distribute upfront)
            // Fees go to treasury and creator via separate withdrawal
        }

        if (payout == 0) revert NothingToClaim();
        _sendToken(m.betToken, msg.sender, payout);

        emit Claimed(marketId, msg.sender, payout);
    }

    /// @notice Creator withdraws their fee share after market is resolved.
    function withdrawCreatorFee(uint256 marketId) external {
        Market storage m = markets[marketId];
        if (m.creator != msg.sender)         revert NotCreator();
        if (m.outcome == Outcome.Unresolved) revert MarketNotResolved();
        if (m.creatorFeeWithdrawn)           revert AlreadyClaimed();
        if (m.outcome == Outcome.Void)       revert NothingToClaim();

        m.creatorFeeWithdrawn = true;
        uint256 totalPot = m.yesPool + m.noPool;
        uint256 creatorFee = (totalPot * CREATOR_FEE_BPS) / 10_000;
        uint256 protocolFee = (totalPot * PROTOCOL_FEE_BPS) / 10_000;

        _sendToken(m.betToken, m.creator,  creatorFee);
        _sendToken(m.betToken, treasury,   protocolFee);
    }

    // ─── View helpers ─────────────────────────────────────────────────────

    /// @notice Estimated payout if you win (before claiming).
    function estimatePayout(uint256 marketId, address bettor) external view returns (uint256) {
        Market storage m = markets[marketId];
        BetRecord storage b = bets[marketId][bettor];
        if (m.outcome == Outcome.Void) return b.yesBet + b.noBet;
        if (m.outcome == Outcome.Unresolved) {
            // Show potential payout if your side wins
            uint256 totalPot = m.yesPool + m.noPool;
            uint256 feeBps   = PROTOCOL_FEE_BPS + CREATOR_FEE_BPS;
            uint256 netPot   = totalPot - (totalPot * feeBps) / 10_000;
            uint256 userBet  = b.yesBet > 0 ? b.yesBet : b.noBet;
            uint256 winPool  = b.yesBet > 0 ? m.yesPool : m.noPool;
            if (winPool == 0) return 0;
            return (netPot * userBet) / winPool;
        }
        uint256 winPool = m.outcome == Outcome.Yes ? m.yesPool : m.noPool;
        uint256 userBet = m.outcome == Outcome.Yes ? b.yesBet  : b.noBet;
        if (winPool == 0 || userBet == 0) return 0;
        uint256 totalPot = m.yesPool + m.noPool;
        uint256 feeBps   = PROTOCOL_FEE_BPS + CREATOR_FEE_BPS;
        uint256 netPot   = totalPot - (totalPot * feeBps) / 10_000;
        return (netPot * userBet) / winPool;
    }

    function getOdds(uint256 marketId) external view returns (uint256 yesOdds, uint256 noOdds) {
        Market storage m = markets[marketId];
        uint256 total = m.yesPool + m.noPool;
        if (total == 0) return (5000, 5000); // 50-50 starting odds
        yesOdds = (m.yesPool * 10_000) / total;
        noOdds  = 10_000 - yesOdds;
    }

    // ─── Internal helpers ─────────────────────────────────────────────────

    function _receiveToken(address token, uint256 amount) private {
        if (token == address(0)) {
            require(msg.value >= amount, "PredMarket: insufficient ZBX");
            if (msg.value > amount) {
                (bool ok,) = msg.sender.call{value: msg.value - amount}("");
                require(ok, "PredMarket: refund failed");
            }
        } else {
            require(IERC20Pred(token).transferFrom(msg.sender, address(this), amount),
                    "PredMarket: transfer failed");
        }
    }

    function _sendToken(address token, address to, uint256 amount) private {
        if (amount == 0) return;
        if (token == address(0)) {
            (bool ok,) = to.call{value: amount}("");
            require(ok, "PredMarket: ZBX send failed");
        } else {
            require(IERC20Pred(token).transfer(to, amount), "PredMarket: send failed");
        }
    }

    receive() external payable {}
}
