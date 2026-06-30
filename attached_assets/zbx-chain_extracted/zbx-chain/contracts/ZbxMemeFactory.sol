// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title  ZbxMemeFactory — pump.fun-style meme coin launchpad on ZBX Chain
/// @author Zebvix Technologies Pvt Ltd
///
/// @notice Anyone can launch a meme coin in one transaction.  No presale.
///         No VC. No rugs. Full fair launch.
///
///         Architecture (pump.fun model):
///           1. Creator pays 0.01 ZBX launch fee → meme coin deployed
///           2. Bonding curve: virtual reserves (constant-product AMM in memory)
///              virtualZbx  = VIRTUAL_ZBX  (30 ZBX)  — invisible liquidity
///              virtualToken = TOTAL_SUPPLY (1 Billion) — all tokens virtual
///           3. Buyers: send ZBX → receive tokens from curve
///              Sellers: send tokens → receive ZBX from curve
///           4. Graduation: when real ZBX raised ≥ GRADUATION_ZBX (30 ZBX)
///              → permanently add liquidity to ZbxAMM, LP tokens burned
///              → creator gets 1% of graduated ZBX, protocol gets 1%
///           5. After graduation: trade on ZbxAMM (no more bonding curve)
///
///         Anti-rug measures:
///           - No token allocation to creator (100% on bonding curve)
///           - LP tokens burned on graduation (permanently locked)
///           - Curve is sealed after graduation
///           - Anti-snipe: first SNIPE_BLOCKS blocks → max 0.5% per TX
///
///         Token model:
///           Total Supply: 1,000,000,000 (1B) tokens (18 decimals)
///           All 1B held by factory on curve at launch
///           ~20% remains on curve at graduation; ~80% circulating
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:module     Meme / ZEP-045

interface IZbxAMM {
    function addLiquidity(
        address tokenA,
        address tokenB,
        uint256 amountADesired,
        uint256 amountBDesired,
        uint256 amountAMin,
        uint256 amountBMin,
        address to,
        uint256 deadline
    ) external payable returns (uint256 amountA, uint256 amountB, uint256 liquidity);
}

interface IMemeToken {
    function transfer(address to, uint256 amount) external returns (bool);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function approve(address spender, uint256 amount) external returns (bool);
    function balanceOf(address account) external view returns (uint256);
    function totalSupply() external view returns (uint256);
    function mint(address to, uint256 amount) external;
    function burn(uint256 amount) external;
}

// ─── Minimal ERC-20 for meme coins ─────────────────────────────────────────

contract MemeToken {
    string  public name;
    string  public symbol;
    uint8   public constant decimals = 18;
    uint256 public totalSupply;

    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    address public factory;
    bool    public tradingEnabled;

    // Social metadata (immutable after creation)
    string  public imageUri;
    string  public description;
    string  public website;
    string  public twitter;
    string  public telegram;

    event Transfer(address indexed from, address indexed to, uint256 value);
    event Approval(address indexed owner, address indexed spender, uint256 value);

    modifier onlyFactory() {
        require(msg.sender == factory, "MemeToken: not factory");
        _;
    }

    constructor(
        string memory name_,
        string memory symbol_,
        string memory imageUri_,
        string memory description_,
        string memory website_,
        string memory twitter_,
        string memory telegram_,
        uint256 supply,
        address factory_
    ) {
        name        = name_;
        symbol      = symbol_;
        imageUri    = imageUri_;
        description = description_;
        website     = website_;
        twitter     = twitter_;
        telegram    = telegram_;
        factory     = factory_;
        totalSupply = supply;
        balanceOf[factory_] = supply;
        emit Transfer(address(0), factory_, supply);
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        _transfer(msg.sender, to, amount);
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        if (allowance[from][msg.sender] != type(uint256).max) {
            allowance[from][msg.sender] -= amount;
        }
        _transfer(from, to, amount);
        return true;
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        emit Approval(msg.sender, spender, amount);
        return true;
    }

    function burn(uint256 amount) external {
        balanceOf[msg.sender] -= amount;
        totalSupply -= amount;
        emit Transfer(msg.sender, address(0), amount);
    }

    function _transfer(address from, address to, uint256 amount) internal {
        require(balanceOf[from] >= amount, "MemeToken: insufficient balance");
        balanceOf[from] -= amount;
        balanceOf[to]   += amount;
        emit Transfer(from, to, amount);
    }
}

import { ReentrancyGuard } from "./libraries/ReentrancyGuard.sol";

// ─── Main Factory ────────────────────────────────────────────────────────────

contract ZbxMemeFactory is ReentrancyGuard {

    // ─── Errors ──────────────────────────────────────────────────────────

    error InsufficientLaunchFee();
    error MemeNotFound();
    error AlreadyGraduated();
    error NotGraduated();
    error SlippageExceeded();
    error ZeroAmount();
    error BuyTooLarge();
    error TransferFailed();
    error NotOwner();
    error ZeroAddress();

    // ─── Events ──────────────────────────────────────────────────────────

    event MemeLaunched(
        uint256 indexed memeId,
        address indexed token,
        address indexed creator,
        string  name,
        string  symbol,
        string  imageUri,
        uint256 launchedAt
    );
    event TokensBought(
        uint256 indexed memeId,
        address indexed buyer,
        uint256 zbxIn,
        uint256 tokensOut,
        uint256 virtualZbx,
        uint256 virtualToken,
        uint256 price
    );
    event TokensSold(
        uint256 indexed memeId,
        address indexed seller,
        uint256 tokensIn,
        uint256 zbxOut,
        uint256 virtualZbx,
        uint256 virtualToken,
        uint256 price
    );
    event Graduated(
        uint256 indexed memeId,
        address indexed token,
        uint256 zbxLiquidity,
        uint256 tokenLiquidity,
        uint256 lpTokensBurned
    );
    event MemeComment(uint256 indexed memeId, address indexed commenter, string comment);
    event MemeLike(uint256 indexed memeId, address indexed liker);

    // ─── Constants ───────────────────────────────────────────────────────

    uint256 public constant TOTAL_SUPPLY       = 1_000_000_000e18;  // 1 billion tokens
    uint256 public constant VIRTUAL_ZBX        = 30e18;             // 30 ZBX virtual liquidity
    uint256 public constant GRADUATION_ZBX     = 30e18;             // graduate at 30 ZBX raised
    uint256 public constant LAUNCH_FEE         = 0.01e18;           // 0.01 ZBX to launch
    uint256 public constant TRADE_FEE_BPS      = 100;               // 1% per trade
    uint256 public constant CREATOR_SHARE_BPS  = 100;               // 1% of graduation to creator
    uint256 public constant PROTOCOL_SHARE_BPS = 100;               // 1% of graduation to protocol
    uint256 public constant SNIPE_BLOCKS       = 5;                 // anti-snipe window
    uint256 public constant MAX_SNIPE_BPS      = 50;                // 0.5% per TX during snipe window
    address public constant DEAD               = 0x000000000000000000000000000000000000dEaD;

    // ─── Types ───────────────────────────────────────────────────────────

    struct Meme {
        address token;
        address creator;
        uint256 launchBlock;
        // Bonding curve state (virtual reserves)
        uint256 virtualZbx;      // starts at VIRTUAL_ZBX, increases with buys
        uint256 virtualToken;    // starts at TOTAL_SUPPLY, decreases with buys
        // Real ZBX raised (not counting virtual)
        uint256 realZbxRaised;
        // Status
        bool    graduated;
        // Stats
        uint256 totalBuys;
        uint256 totalSells;
        uint256 totalVolume;     // in ZBX
    }

    // ─── State ───────────────────────────────────────────────────────────

    address public owner;
    address public treasury;
    address public ammRouter;    // ZbxAMM router for graduation

    mapping(uint256 => Meme) public memes;
    uint256 public memeCount;

    mapping(address => uint256) public tokenToMemeId;

    // Fee collection
    uint256 public feeBalance;

    // ─── Constructor ─────────────────────────────────────────────────────

    constructor(address treasury_, address ammRouter_) {
        require(treasury_ != address(0), "Factory: zero treasury");
        owner     = msg.sender;
        treasury  = treasury_;
        ammRouter = ammRouter_;
    }

    // ─── Launch meme coin ────────────────────────────────────────────────

    /// @notice Launch a new meme coin.  Pay LAUNCH_FEE ZBX.
    ///         All 1B tokens go to the bonding curve — zero to creator.
    ///
    /// @param  name_        Token name (e.g. "Doge But Better")
    /// @param  symbol_      Token symbol (e.g. "DOGEBTTR")
    /// @param  imageUri_    IPFS or HTTPS URI for the meme image
    /// @param  description_ Short description / tagline
    /// @param  website_     Optional website URL
    /// @param  twitter_     Optional Twitter/X handle
    /// @param  telegram_    Optional Telegram link
    function launchMeme(
        string calldata name_,
        string calldata symbol_,
        string calldata imageUri_,
        string calldata description_,
        string calldata website_,
        string calldata twitter_,
        string calldata telegram_
    ) external payable returns (uint256 memeId, address token) {
        if (msg.value < LAUNCH_FEE) revert InsufficientLaunchFee();

        // Deploy meme token (all supply to this contract)
        MemeToken t = new MemeToken(
            name_, symbol_, imageUri_, description_, website_, twitter_, telegram_,
            TOTAL_SUPPLY,
            address(this)
        );
        token = address(t);

        memeId = ++memeCount;
        memes[memeId] = Meme({
            token:         token,
            creator:       msg.sender,
            launchBlock:   block.number,
            virtualZbx:    VIRTUAL_ZBX,
            virtualToken:  TOTAL_SUPPLY,
            realZbxRaised: 0,
            graduated:     false,
            totalBuys:     0,
            totalSells:    0,
            totalVolume:   0
        });
        tokenToMemeId[token] = memeId;

        // Protocol keeps launch fee
        feeBalance += msg.value;

        emit MemeLaunched(memeId, token, msg.sender, name_, symbol_, imageUri_, block.timestamp);
    }

    // ─── Buy tokens from bonding curve ───────────────────────────────────

    /// @notice Buy meme tokens with ZBX.
    ///         Price increases as more tokens are bought.
    /// @param  memeId   Meme coin identifier.
    /// @param  minOut   Minimum tokens to receive (slippage protection).
    // SOL-06 (MEDIUM): nonReentrant added — buy() transfers tokens to caller
    // and calls _graduate() which sends ZBX. A malicious meme token or AMM
    // router could re-enter and manipulate bonding curve reserves.
    function buy(uint256 memeId, uint256 minOut) external payable nonReentrant returns (uint256 tokensOut) {
        if (msg.value == 0) revert ZeroAmount();
        Meme storage m = _validActive(memeId);

        // Deduct fee
        uint256 fee     = (msg.value * TRADE_FEE_BPS) / 10_000;
        uint256 zbxIn   = msg.value - fee;
        feeBalance += fee;

        // Constant product: (virtualZbx + zbxIn) * (virtualToken - tokensOut) = k
        // tokensOut = virtualToken * zbxIn / (virtualZbx + zbxIn)
        tokensOut = (m.virtualToken * zbxIn) / (m.virtualZbx + zbxIn);
        if (tokensOut == 0) revert ZeroAmount();
        if (tokensOut < minOut) revert SlippageExceeded();

        // Anti-snipe: first SNIPE_BLOCKS → max 0.5% of supply per TX
        if (block.number <= m.launchBlock + SNIPE_BLOCKS) {
            uint256 maxSnipe = (TOTAL_SUPPLY * MAX_SNIPE_BPS) / 10_000;
            if (tokensOut > maxSnipe) revert BuyTooLarge();
        }

        // Update virtual reserves
        m.virtualZbx    += zbxIn;
        m.virtualToken  -= tokensOut;
        m.realZbxRaised += zbxIn;
        m.totalBuys++;
        m.totalVolume   += msg.value;

        // Transfer tokens to buyer
        MemeToken(m.token).transfer(msg.sender, tokensOut);

        uint256 price = (m.virtualZbx * 1e18) / m.virtualToken;
        emit TokensBought(memeId, msg.sender, zbxIn, tokensOut, m.virtualZbx, m.virtualToken, price);

        // Auto-graduate when threshold reached
        if (m.realZbxRaised >= GRADUATION_ZBX && !m.graduated) {
            _graduate(memeId, m);
        }
    }

    // ─── Sell tokens back to bonding curve ───────────────────────────────

    /// @notice Sell meme tokens back to the bonding curve for ZBX.
    ///         Price decreases as tokens are sold.
    /// @param  memeId     Meme coin identifier.
    /// @param  tokenAmount Amount of tokens to sell.
    /// @param  minZbxOut  Minimum ZBX to receive (slippage protection).
    // SOL-06 (MEDIUM): nonReentrant added — sell() sends native ZBX via
    // .call to msg.sender. A malicious contract could re-enter sell() or
    // buy() to drain reserves before state is consistent.
    function sell(uint256 memeId, uint256 tokenAmount, uint256 minZbxOut) external nonReentrant returns (uint256 zbxOut) {
        if (tokenAmount == 0) revert ZeroAmount();
        Meme storage m = _validActive(memeId);

        // Constant product: zbxOut = virtualZbx * tokenAmount / (virtualToken + tokenAmount)
        zbxOut = (m.virtualZbx * tokenAmount) / (m.virtualToken + tokenAmount);
        if (zbxOut == 0) revert ZeroAmount();

        // Deduct fee
        uint256 fee    = (zbxOut * TRADE_FEE_BPS) / 10_000;
        uint256 netOut = zbxOut - fee;
        if (netOut < minZbxOut) revert SlippageExceeded();

        feeBalance += fee;

        // Receive tokens from seller
        require(MemeToken(m.token).transferFrom(msg.sender, address(this), tokenAmount),
                "Factory: token transfer failed");

        // Update virtual reserves
        m.virtualZbx   -= zbxOut;
        m.virtualToken += tokenAmount;
        if (m.realZbxRaised >= zbxOut) m.realZbxRaised -= zbxOut;
        else m.realZbxRaised = 0;
        m.totalSells++;
        m.totalVolume += zbxOut;

        // Send ZBX to seller
        (bool ok,) = msg.sender.call{value: netOut}("");
        require(ok, "Factory: ZBX send failed");

        uint256 price = (m.virtualZbx * 1e18) / m.virtualToken;
        emit TokensSold(memeId, msg.sender, tokenAmount, netOut, m.virtualZbx, m.virtualToken, price);
    }

    // ─── Graduation ──────────────────────────────────────────────────────

    /// @dev Internal graduation: add liquidity to ZbxAMM, burn LP tokens.
    function _graduate(uint256 memeId, Meme storage m) private {
        m.graduated = true;

        uint256 totalZbx      = m.realZbxRaised;
        uint256 creatorShare  = (totalZbx * CREATOR_SHARE_BPS)  / 10_000;
        uint256 protocolShare = (totalZbx * PROTOCOL_SHARE_BPS) / 10_000;
        uint256 liquidityZbx  = totalZbx - creatorShare - protocolShare;

        // Tokens remaining in factory for this meme = virtualToken balance
        uint256 liquidityToken = m.virtualToken;

        // Send creator + protocol shares
        (bool ok1,) = m.creator.call{value: creatorShare}("");
        require(ok1, "Factory: creator share failed");
        feeBalance += protocolShare;

        // Approve AMM to take tokens
        MemeToken(m.token).approve(ammRouter, liquidityToken);

        // Add liquidity (LP tokens go to DEAD address — permanently locked)
        if (ammRouter != address(0) && liquidityZbx > 0 && liquidityToken > 0) {
            try IZbxAMM(ammRouter).addLiquidity{value: liquidityZbx}(
                m.token,
                address(0),    // native ZBX pair
                liquidityToken,
                liquidityZbx,
                0,
                0,
                DEAD,          // LP tokens burned
                block.timestamp + 300
            ) returns (uint256, uint256, uint256 lp) {
                emit Graduated(memeId, m.token, liquidityZbx, liquidityToken, lp);
            } catch {
                // AMM not set or call failed — keep ZBX + tokens in factory
                // Emit with 0 lp to signal partial graduation
                emit Graduated(memeId, m.token, liquidityZbx, liquidityToken, 0);
            }
        } else {
            emit Graduated(memeId, m.token, liquidityZbx, liquidityToken, 0);
        }
    }

    // ─── Social features ─────────────────────────────────────────────────

    /// @notice Leave an on-chain comment for a meme coin.
    function comment(uint256 memeId, string calldata text) external {
        if (memes[memeId].token == address(0)) revert MemeNotFound();
        emit MemeComment(memeId, msg.sender, text);
    }

    /// @notice Like a meme coin (on-chain signal, no token required).
    function like(uint256 memeId) external {
        if (memes[memeId].token == address(0)) revert MemeNotFound();
        emit MemeLike(memeId, msg.sender);
    }

    // ─── View helpers ────────────────────────────────────────────────────

    /// @notice Current token price in ZBX (18-decimal).
    function currentPrice(uint256 memeId) external view returns (uint256) {
        Meme storage m = memes[memeId];
        if (m.token == address(0)) revert MemeNotFound();
        return (m.virtualZbx * 1e18) / m.virtualToken;
    }

    /// @notice Market cap in ZBX (circulating supply × price).
    function marketCap(uint256 memeId) external view returns (uint256) {
        Meme storage m = memes[memeId];
        if (m.token == address(0)) revert MemeNotFound();
        uint256 circulating = TOTAL_SUPPLY - m.virtualToken;
        uint256 price       = (m.virtualZbx * 1e18) / m.virtualToken;
        return (circulating * price) / 1e18;
    }

    /// @notice Simulate a buy: how many tokens for `zbxAmount` ZBX?
    function quoteBuy(uint256 memeId, uint256 zbxAmount) external view
        returns (uint256 tokensOut, uint256 newPrice)
    {
        Meme storage m = memes[memeId];
        if (m.token == address(0)) revert MemeNotFound();
        uint256 fee   = (zbxAmount * TRADE_FEE_BPS) / 10_000;
        uint256 zbxIn = zbxAmount - fee;
        tokensOut = (m.virtualToken * zbxIn) / (m.virtualZbx + zbxIn);
        newPrice  = ((m.virtualZbx + zbxIn) * 1e18) / (m.virtualToken - tokensOut);
    }

    /// @notice Simulate a sell: how much ZBX for `tokenAmount` tokens?
    function quoteSell(uint256 memeId, uint256 tokenAmount) external view
        returns (uint256 zbxOut, uint256 newPrice)
    {
        Meme storage m = memes[memeId];
        if (m.token == address(0)) revert MemeNotFound();
        uint256 gross = (m.virtualZbx * tokenAmount) / (m.virtualToken + tokenAmount);
        uint256 fee   = (gross * TRADE_FEE_BPS) / 10_000;
        zbxOut   = gross - fee;
        newPrice = ((m.virtualZbx - gross) * 1e18) / (m.virtualToken + tokenAmount);
    }

    /// @notice Progress toward graduation (0–10000 bps).
    function graduationProgress(uint256 memeId) external view returns (uint256 bps) {
        Meme storage m = memes[memeId];
        if (m.token == address(0)) revert MemeNotFound();
        if (m.graduated) return 10_000;
        bps = (m.realZbxRaised * 10_000) / GRADUATION_ZBX;
        if (bps > 10_000) bps = 10_000;
    }

    /// @notice Paginated list of all meme coins.
    function listMemes(uint256 offset, uint256 limit)
        external view
        returns (uint256[] memory ids, address[] memory tokens, bool[] memory graduated_)
    {
        uint256 total = memeCount;
        if (offset >= total) {
            return (new uint256[](0), new address[](0), new bool[](0));
        }
        uint256 end   = offset + limit > total ? total : offset + limit;
        uint256 count = end - offset;
        ids        = new uint256[](count);
        tokens     = new address[](count);
        graduated_ = new bool[](count);
        for (uint256 i; i < count; i++) {
            uint256 id = total - offset - i;   // newest first
            ids[i]        = id;
            tokens[i]     = memes[id].token;
            graduated_[i] = memes[id].graduated;
        }
    }

    // ─── Admin ───────────────────────────────────────────────────────────

    function withdrawFees() external {
        require(msg.sender == owner, "Factory: not owner");
        uint256 amount = feeBalance;
        feeBalance = 0;
        (bool ok,) = treasury.call{value: amount}("");
        require(ok, "Factory: fee withdrawal failed");
    }

    function setAmmRouter(address router_) external {
        require(msg.sender == owner, "Factory: not owner");
        ammRouter = router_;
    }

    function setTreasury(address t) external {
        require(msg.sender == owner, "Factory: not owner");
        require(t != address(0), "Factory: zero treasury");
        treasury = t;
    }

    // ─── Internal ────────────────────────────────────────────────────────

    function _validActive(uint256 memeId) private view returns (Meme storage m) {
        m = memes[memeId];
        if (m.token == address(0)) revert MemeNotFound();
        if (m.graduated)           revert AlreadyGraduated();
    }

    receive() external payable {}
}
