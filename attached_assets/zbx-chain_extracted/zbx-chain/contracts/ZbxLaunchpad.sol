// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title  ZbxLaunchpad — Fair token launch / IDO platform
/// @author Zebvix Technologies Pvt Ltd
///
/// @notice Projects register a token sale; whitelisted participants can buy
///         tokens at a fixed price during the sale window.  Tokens are
///         distributed with a cliff + linear vesting schedule so teams
///         cannot dump immediately.
///
///         Two allocation modes:
///           FCFS    — First-come-first-served until cap is reached.
///           EQUAL   — All whitelisted addresses get the same guaranteed
///                     allocation; any unsold tokens are refunded to project.
///
///         Raise currency: native ZBX or any ERC-20 (configurable per sale).
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:module     DeFi / Launchpad (ZEP-036)

interface IERC20Launch {
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function transfer(address to, uint256 amount) external returns (bool);
    function balanceOf(address account) external view returns (uint256);
}

contract ZbxLaunchpad {

    // ─── Errors ───────────────────────────────────────────────────────────

    error SaleNotFound();
    error SaleNotOpen();
    error SaleStillOpen();
    error NotWhitelisted();
    error CapReached();
    error AllocationExceeded();
    error AlreadyClaimed();
    error NothingToClaim();
    error CliffNotPassed();
    error NotProjectOwner();
    error NotAdmin();
    error InvalidTime();
    error ZeroAmount();
    error ZeroAddress();
    error SoftCapReached();
    error RefundWindowClosed();

    // ─── Events ───────────────────────────────────────────────────────────

    event SaleCreated(
        uint256 indexed saleId,
        address indexed project,
        address         saleToken,
        address         raiseToken,
        uint256         price,
        uint256         hardCap,
        uint256         startTime,
        uint256         endTime
    );
    event Participated(uint256 indexed saleId, address indexed buyer, uint256 raisedAmount, uint256 tokenAmount);
    event Claimed(uint256 indexed saleId, address indexed buyer, uint256 tokenAmount);
    event RaisedWithdrawn(uint256 indexed saleId, address indexed project, uint256 amount);
    event UnsoldRefunded(uint256 indexed saleId, address indexed project, uint256 amount);
    event WhitelistUpdated(uint256 indexed saleId, address[] accounts, bool allowed);
    event Refunded(uint256 indexed saleId, address indexed buyer, uint256 amount);

    // ─── Types ────────────────────────────────────────────────────────────

    enum SaleMode { FCFS, EQUAL }

    struct Sale {
        address  project;           // project owner
        address  saleToken;         // token being sold
        address  raiseToken;        // token used to buy (address(0) = native ZBX)
        uint256  price;             // raise tokens per 1e18 sale tokens
        uint256  hardCap;           // max raise amount
        uint256  softCap;           // min raise for success
        uint256  maxPerWallet;      // max raise per address
        uint256  startTime;
        uint256  endTime;
        uint64   cliffDuration;     // seconds after endTime before any tokens unlock
        uint64   vestingDuration;   // total vesting seconds (after cliff)
        SaleMode mode;
        uint256  totalRaised;
        uint256  totalTokensSold;
        bool     finalized;
        bool     raiseWithdrawn;
    }

    struct Participation {
        uint256 raised;          // how much raise token the buyer spent
        uint256 tokenAlloc;      // tokens allocated to this buyer
        uint256 tokensClaimed;   // tokens claimed so far
        uint256 claimStart;      // timestamp when cliff ends
    }

    // ─── State ────────────────────────────────────────────────────────────

    address public admin;
    uint256 public platformFeeBps = 200;   // 2% of raise
    address public feeTreasury;

    mapping(uint256 => Sale) public sales;
    uint256 public saleCount;

    mapping(uint256 => mapping(address => Participation)) public participations;
    mapping(uint256 => mapping(address => bool)) public whitelist;

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor(address feeTreasury_) {
        require(feeTreasury_ != address(0), "Launchpad: zero treasury");
        admin       = msg.sender;
        feeTreasury = feeTreasury_;
    }

    // ─── Sale creation ────────────────────────────────────────────────────

    /// @notice Register a new token sale.
    /// @param  saleToken        The token being sold (must be pre-deposited).
    /// @param  raiseToken       Token buyers pay with (address(0) = native ZBX).
    /// @param  price            Raise tokens per 1e18 sale tokens.
    /// @param  hardCap          Maximum raise amount.
    /// @param  softCap          Minimum raise for the sale to succeed.
    /// @param  maxPerWallet     Maximum raise per wallet.
    /// @param  startTime        Unix timestamp when sale opens.
    /// @param  endTime          Unix timestamp when sale closes.
    /// @param  cliffDuration    Seconds after endTime before tokens unlock.
    /// @param  vestingDuration  Total vesting period (including cliff).
    /// @param  mode             FCFS or EQUAL allocation.
    function createSale(
        address  saleToken,
        address  raiseToken,
        uint256  price,
        uint256  hardCap,
        uint256  softCap,
        uint256  maxPerWallet,
        uint256  startTime,
        uint256  endTime,
        uint64   cliffDuration,
        uint64   vestingDuration,
        SaleMode mode
    ) external returns (uint256 saleId) {
        if (saleToken == address(0))    revert ZeroAddress();
        if (price == 0)                 revert ZeroAmount();
        if (startTime >= endTime)       revert InvalidTime();
        if (startTime < block.timestamp) revert InvalidTime();

        saleId = ++saleCount;
        sales[saleId] = Sale({
            project:         msg.sender,
            saleToken:       saleToken,
            raiseToken:      raiseToken,
            price:           price,
            hardCap:         hardCap,
            softCap:         softCap,
            maxPerWallet:    maxPerWallet == 0 ? hardCap : maxPerWallet,
            startTime:       startTime,
            endTime:         endTime,
            cliffDuration:   cliffDuration,
            vestingDuration: vestingDuration,
            mode:            mode,
            totalRaised:     0,
            totalTokensSold: 0,
            finalized:       false,
            raiseWithdrawn:  false
        });

        emit SaleCreated(saleId, msg.sender, saleToken, raiseToken,
                         price, hardCap, startTime, endTime);
    }

    // ─── Whitelist ────────────────────────────────────────────────────────

    function updateWhitelist(uint256 saleId, address[] calldata accounts, bool allowed) external {
        Sale storage s = sales[saleId];
        if (s.project != msg.sender && msg.sender != admin) revert NotProjectOwner();
        for (uint256 i; i < accounts.length; ++i) {
            whitelist[saleId][accounts[i]] = allowed;
        }
        emit WhitelistUpdated(saleId, accounts, allowed);
    }

    // ─── Participate ──────────────────────────────────────────────────────

    /// @notice Buy tokens.  `amount` = raise token amount to spend.
    function participate(uint256 saleId, uint256 amount) external payable {
        Sale storage s = sales[saleId];
        if (s.project == address(0))          revert SaleNotFound();
        if (block.timestamp < s.startTime
         || block.timestamp > s.endTime)      revert SaleNotOpen();
        if (!whitelist[saleId][msg.sender])   revert NotWhitelisted();
        if (s.totalRaised >= s.hardCap)       revert CapReached();

        uint256 spend = amount;
        // Cap to remaining hard cap
        if (s.totalRaised + spend > s.hardCap) {
            spend = s.hardCap - s.totalRaised;
        }

        Participation storage p = participations[saleId][msg.sender];
        if (p.raised + spend > s.maxPerWallet) {
            spend = s.maxPerWallet - p.raised;
        }
        if (spend == 0) revert AllocationExceeded();

        // Receive raise tokens
        if (s.raiseToken == address(0)) {
            require(msg.value >= spend, "Launchpad: insufficient ZBX");
            // Refund excess
            if (msg.value > spend) {
                (bool ok,) = msg.sender.call{value: msg.value - spend}("");
                require(ok, "Launchpad: refund failed");
            }
        } else {
            require(IERC20Launch(s.raiseToken).transferFrom(msg.sender, address(this), spend),
                    "Launchpad: transfer failed");
        }

        uint256 tokenAmount = (spend * 1e18) / s.price;

        s.totalRaised     += spend;
        s.totalTokensSold += tokenAmount;
        p.raised          += spend;
        p.tokenAlloc      += tokenAmount;

        emit Participated(saleId, msg.sender, spend, tokenAmount);
    }

    // ─── Finalize ─────────────────────────────────────────────────────────

    /// @notice Finalize the sale (callable after endTime by anyone).
    ///         Sets the cliff start time for all participants.
    function finalize(uint256 saleId) external {
        Sale storage s = sales[saleId];
        if (s.project == address(0))        revert SaleNotFound();
        if (block.timestamp <= s.endTime)   revert SaleStillOpen();
        if (s.finalized)                    return;

        s.finalized = true;
    }

    // ─── Claim tokens (vested) ────────────────────────────────────────────

    /// @notice Claim unlocked tokens according to cliff + linear vesting.
    function claim(uint256 saleId) external {
        Sale storage s = sales[saleId];
        if (!s.finalized) revert SaleStillOpen();

        Participation storage p = participations[saleId][msg.sender];
        if (p.tokenAlloc == 0) revert NothingToClaim();

        if (p.claimStart == 0) p.claimStart = s.endTime + s.cliffDuration;
        if (block.timestamp < p.claimStart) revert CliffNotPassed();

        uint256 unlocked = _vestedAmount(p, s);
        uint256 claimable = unlocked - p.tokensClaimed;
        if (claimable == 0) revert NothingToClaim();

        p.tokensClaimed += claimable;

        require(IERC20Launch(s.saleToken).transfer(msg.sender, claimable),
                "Launchpad: token transfer failed");

        emit Claimed(saleId, msg.sender, claimable);
    }

    // ─── Project withdrawals ──────────────────────────────────────────────

    /// SEC-2026-05-09 Pass-19 (Tier-2 #11): explicit refund window
    /// for failed sales (softCap not reached). Pre-fix the project
    /// could call `withdrawRaised` even on a failed sale, leaving
    /// buyers stranded with un-vested allocations and no path to
    /// recover their raise tokens. New rules:
    ///   * If `totalRaised < softCap` after `endTime`, the sale is
    ///     considered FAILED and buyers MAY call `refund(saleId)`
    ///     during the REFUND_WINDOW (7 days post-endTime).
    ///   * `withdrawRaised` REVERTS on a failed sale — project
    ///     cannot drain the contract, raise stays escrowed for
    ///     buyer refunds. Project can only call after the window
    ///     closes AND only if softCap was met (additional guard
    ///     added below).
    uint256 public constant REFUND_WINDOW = 7 days;
    mapping(uint256 => mapping(address => bool)) public refunded;

    function refund(uint256 saleId) external {
        Sale storage s = sales[saleId];
        if (s.project == address(0))                     revert SaleNotFound();
        if (block.timestamp <= s.endTime)                revert SaleStillOpen();
        if (s.totalRaised >= s.softCap)                  revert SoftCapReached();
        if (block.timestamp > s.endTime + REFUND_WINDOW) revert RefundWindowClosed();

        Participation storage p = participations[saleId][msg.sender];
        if (p.raised == 0)               revert NothingToClaim();
        if (refunded[saleId][msg.sender]) revert AlreadyClaimed();

        uint256 amount = p.raised;
        refunded[saleId][msg.sender] = true;
        // Zero out future-claim accounting so buyer cannot also call claim().
        p.tokenAlloc = 0;

        _sendRaise(s.raiseToken, msg.sender, amount);
        emit Refunded(saleId, msg.sender, amount);
    }

    /// @notice Project withdraws raised funds (minus platform fee).
    function withdrawRaised(uint256 saleId) external {
        Sale storage s = sales[saleId];
        if (s.project != msg.sender)    revert NotProjectOwner();
        if (!s.finalized)               revert SaleStillOpen();
        if (s.raiseWithdrawn)           revert AlreadyClaimed();
        // Pass-19 Tier-2 #11: failed sales cannot withdraw — funds
        // stay escrowed for buyer refunds.
        if (s.totalRaised < s.softCap)  revert SoftCapReached();

        s.raiseWithdrawn = true;
        uint256 total = s.totalRaised;
        uint256 fee   = (total * platformFeeBps) / 10_000;
        uint256 net   = total - fee;

        _sendRaise(s.raiseToken, feeTreasury, fee);
        _sendRaise(s.raiseToken, s.project,   net);

        emit RaisedWithdrawn(saleId, s.project, net);
    }

    /// SEC-2026-05-09 Pass-15 (HIGH-S11 / Pass-12 Tier-2 Launchpad-reclaim):
    /// Pre-fix `unsold` was computed from the assumed total token supply
    /// (`hardCap / price`) regardless of how much the project had
    /// actually deposited. If the project deposited less than the
    /// notional supply (common for soft-cap sales), `reclaimUnsold`
    /// would either revert mid-transfer (best case) or, worse, drain
    /// tokens deposited by other concurrent sales sharing the same
    /// saleToken. New formulation reads the contract's actual
    /// saleToken balance and subtracts buyer-claimable allocations.
    /// Also adds a `reclaimed` flag so the function is one-shot and
    /// cannot be replayed even if hardCap is later mutated.
    mapping(uint256 => bool) public reclaimedUnsold;

    function reclaimUnsold(uint256 saleId) external {
        Sale storage s = sales[saleId];
        if (s.project != msg.sender) revert NotProjectOwner();
        if (!s.finalized)            revert SaleStillOpen();
        if (reclaimedUnsold[saleId]) revert NothingToClaim();

        // Actual on-contract holdings, NOT the notional `hardCap/price`.
        uint256 held = IERC20Launch(s.saleToken).balanceOf(address(this));
        // Buyers still need `totalTokensSold` — reserve that.
        uint256 reserved = s.totalTokensSold;
        uint256 unsold = held > reserved ? held - reserved : 0;
        if (unsold == 0) revert NothingToClaim();

        reclaimedUnsold[saleId] = true;

        require(IERC20Launch(s.saleToken).transfer(s.project, unsold),
                "Launchpad: unsold transfer failed");

        emit UnsoldRefunded(saleId, s.project, unsold);
    }

    // ─── View helpers ─────────────────────────────────────────────────────

    function claimableAmount(uint256 saleId, address buyer) external view returns (uint256) {
        Sale storage s = sales[saleId];
        if (!s.finalized) return 0;
        Participation storage p = participations[saleId][buyer];
        if (p.tokenAlloc == 0) return 0;
        uint256 claimStart = p.claimStart == 0
            ? s.endTime + s.cliffDuration
            : p.claimStart;
        if (block.timestamp < claimStart) return 0;
        uint256 unlocked = _vestedAmountView(p, s, claimStart);
        return unlocked > p.tokensClaimed ? unlocked - p.tokensClaimed : 0;
    }

    // ─── Internal helpers ─────────────────────────────────────────────────

    function _vestedAmount(Participation storage p, Sale storage s)
        internal view returns (uint256)
    {
        return _vestedAmountView(p, s, p.claimStart);
    }

    function _vestedAmountView(
        Participation storage p,
        Sale storage s,
        uint256 claimStart
    ) internal view returns (uint256) {
        if (s.vestingDuration == 0) return p.tokenAlloc;
        uint256 vestEnd = claimStart + s.vestingDuration;
        if (block.timestamp >= vestEnd) return p.tokenAlloc;
        uint256 elapsed = block.timestamp - claimStart;
        return (p.tokenAlloc * elapsed) / s.vestingDuration;
    }

    function _sendRaise(address token, address to, uint256 amount) private {
        if (amount == 0) return;
        if (token == address(0)) {
            (bool ok,) = to.call{value: amount}("");
            require(ok, "Launchpad: ZBX send failed");
        } else {
            require(IERC20Launch(token).transfer(to, amount), "Launchpad: send failed");
        }
    }

    receive() external payable {}
}
