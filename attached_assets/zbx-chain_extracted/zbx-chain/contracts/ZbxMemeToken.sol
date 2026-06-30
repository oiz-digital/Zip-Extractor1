// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title  ZbxMemeToken — Advanced standalone meme coin with tax + reflection + burn
/// @author Zebvix Technologies Pvt Ltd
///
/// @notice For users who want to deploy a meme coin with advanced tokenomics
///         WITHOUT the bonding curve (direct deploy + own liquidity management).
///
///         Features:
///           • ERC-20 with configurable buy/sell/transfer taxes
///           • Auto-burn: portion of every tax burned permanently
///           • Reflection: portion distributed to all holders (proportional)
///           • Max wallet: prevents whale accumulation
///           • Max transaction: prevents single large dumps
///           • Blacklist: anti-bot protection (owner-controlled)
///           • Anti-snipe: automatic max TX limit for first N blocks
///           • Renounce ownership: owner can permanently give up control
///           • Tax-free addresses: liquidity pair + router (configurable)
///
///         Tokenomics example:
///           Buy tax:     4% (2% burn + 2% reflection)
///           Sell tax:    6% (3% burn + 2% reflection + 1% dev)
///           Transfer:    0%
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:module     Meme / ZEP-045

contract ZbxMemeToken {

    // ─── Errors ───────────────────────────────────────────────────────────

    error NotOwner();
    error Blacklisted();
    error MaxWalletExceeded();
    error MaxTxExceeded();
    error TradingNotEnabled();
    error OwnerRenounced();
    error InvalidTax();
    error ZeroAddress();

    // ─── Events ───────────────────────────────────────────────────────────

    event Transfer(address indexed from, address indexed to, uint256 value);
    event Approval(address indexed owner, address indexed spender, uint256 value);
    event TaxUpdated(uint8 buyBurn, uint8 buyReflect, uint8 buyDev,
                     uint8 sellBurn, uint8 sellReflect, uint8 sellDev);
    event MaxWalletUpdated(uint256 newMax);
    event MaxTxUpdated(uint256 newMax);
    event TradingEnabled(uint256 block_);
    event OwnershipRenounced();
    event BlacklistUpdated(address indexed account, bool blacklisted);
    event TaxExemptUpdated(address indexed account, bool exempt);
    event LiquidityPairSet(address indexed pair, bool isPair);
    event Reflected(uint256 amount);
    event Burned(uint256 amount);

    // ─── Storage ──────────────────────────────────────────────────────────

    // Token basics
    string  public name;
    string  public symbol;
    uint8   public constant decimals = 18;
    uint256 public totalSupply;

    // Social metadata
    string  public imageUri;
    string  public description;
    string  public website;
    string  public twitter;
    string  public telegram;

    // ERC-20 state
    mapping(address => uint256) private _balance;
    mapping(address => mapping(address => uint256)) public allowance;

    // Reflection (rTokens = reflected tokens for proportional share)
    mapping(address => uint256) private _rBalance;
    uint256 private _rTotal;          // total reflected supply
    uint256 private _tTotal;          // total actual supply (decreases with burns)
    bool    public  reflectionEnabled;

    // Tax configuration (in basis points, max 25% total per type)
    struct TaxConfig {
        uint8 burn;      // % burned
        uint8 reflect;   // % distributed to holders
        uint8 dev;       // % to dev wallet
    }
    TaxConfig public buyTax;
    TaxConfig public sellTax;
    TaxConfig public transferTax;   // usually 0

    address public devWallet;

    // Limits
    uint256 public maxWallet;     // max tokens per wallet (0 = disabled)
    uint256 public maxTx;         // max tokens per transaction (0 = disabled)

    // Trading gates
    bool    public tradingEnabled;
    uint256 public tradingBlock;

    // Anti-snipe
    uint256 public constant SNIPE_BLOCKS = 3;
    uint256 public snipeMaxTx;

    // Ownership + control
    address public owner;
    bool    public ownershipRenounced;

    // Lookup sets
    mapping(address => bool) public isBlacklisted;
    mapping(address => bool) public isTaxExempt;
    mapping(address => bool) public isLiquidityPair;  // buy = from pair, sell = to pair

    address public constant DEAD = 0x000000000000000000000000000000000000dEaD;

    // ─── Constructor ──────────────────────────────────────────────────────

    /// @param supply_   Total supply in whole tokens (e.g. 1_000_000_000 for 1B)
    /// @param reflection_ Enable holder reflection
    constructor(
        string memory name_,
        string memory symbol_,
        string memory imageUri_,
        string memory description_,
        string memory website_,
        string memory twitter_,
        string memory telegram_,
        uint256 supply_,
        address devWallet_,
        bool    reflection_
    ) {
        require(devWallet_ != address(0), "MemeToken: zero dev wallet");

        name        = name_;
        symbol      = symbol_;
        imageUri    = imageUri_;
        description = description_;
        website     = website_;
        twitter     = twitter_;
        telegram    = telegram_;
        devWallet   = devWallet_;
        owner       = msg.sender;
        reflectionEnabled = reflection_;

        uint256 supply = supply_ * 1e18;
        totalSupply = supply;
        _tTotal     = supply;

        if (reflection_) {
            // rTotal: very large number divisible by totalSupply
            _rTotal = (~uint256(0) - (~uint256(0) % supply));
            _rBalance[msg.sender] = _rTotal;
        } else {
            _balance[msg.sender] = supply;
        }

        // Default taxes: 2% buy (1% burn + 1% reflect), 4% sell (2% burn + 2% reflect)
        buyTax      = TaxConfig(1, 1, 0);
        sellTax     = TaxConfig(2, 2, 0);
        transferTax = TaxConfig(0, 0, 0);

        // Default limits: 2% max wallet, 1% max TX, 0.5% snipe
        maxWallet   = (supply * 200) / 10_000;
        maxTx       = (supply * 100) / 10_000;
        snipeMaxTx  = (supply * 50)  / 10_000;

        isTaxExempt[msg.sender] = true;
        isTaxExempt[address(this)] = true;
        isTaxExempt[DEAD] = true;

        emit Transfer(address(0), msg.sender, supply);
    }

    // ─── ERC-20 ───────────────────────────────────────────────────────────

    function balanceOf(address account) public view returns (uint256) {
        if (reflectionEnabled && !isTaxExempt[account]) {
            return _reflectToToken(_rBalance[account]);
        }
        return _balance[account];
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

    // ─── Internal transfer ────────────────────────────────────────────────

    function _transfer(address from, address to, uint256 amount) internal {
        if (isBlacklisted[from] || isBlacklisted[to]) revert Blacklisted();
        if (amount == 0) return;

        // Trading gate
        if (!tradingEnabled && !isTaxExempt[from] && !isTaxExempt[to]) {
            revert TradingNotEnabled();
        }

        // Anti-snipe: first SNIPE_BLOCKS → enforce tighter maxTx
        if (tradingEnabled && block.number <= tradingBlock + SNIPE_BLOCKS) {
            if (!isTaxExempt[from] && !isTaxExempt[to]) {
                if (amount > snipeMaxTx) revert MaxTxExceeded();
            }
        }

        // Max TX
        if (maxTx > 0 && !isTaxExempt[from]) {
            if (amount > maxTx) revert MaxTxExceeded();
        }

        // Determine tax config
        TaxConfig memory taxCfg;
        if (isLiquidityPair[from] && !isTaxExempt[to]) {
            taxCfg = buyTax;   // buy from DEX
        } else if (isLiquidityPair[to] && !isTaxExempt[from]) {
            taxCfg = sellTax;  // sell to DEX
        } else if (!isTaxExempt[from] && !isTaxExempt[to]) {
            taxCfg = transferTax;
        }

        uint256 totalTaxBps = uint256(taxCfg.burn) + taxCfg.reflect + taxCfg.dev;
        uint256 taxAmount   = (amount * totalTaxBps) / 100;
        uint256 netAmount   = amount - taxAmount;

        // Apply tax splits
        if (taxAmount > 0) {
            uint256 burnAmt    = (taxAmount * taxCfg.burn)    / totalTaxBps;
            uint256 reflectAmt = (taxAmount * taxCfg.reflect) / totalTaxBps;
            uint256 devAmt     = taxAmount - burnAmt - reflectAmt;

            if (burnAmt > 0)    _burn(from, burnAmt);
            if (reflectAmt > 0) _reflect(from, reflectAmt);
            if (devAmt > 0)     _move(from, devWallet, devAmt);
        }

        _move(from, to, netAmount);

        // Max wallet check (post-transfer)
        if (maxWallet > 0 && !isTaxExempt[to] && !isLiquidityPair[to]) {
            if (balanceOf(to) > maxWallet) revert MaxWalletExceeded();
        }
    }

    // ─── Reflection internals ─────────────────────────────────────────────

    function _move(address from, address to, uint256 tAmount) private {
        if (reflectionEnabled) {
            uint256 rAmount = _tokenToReflect(tAmount);
            _rBalance[from] -= rAmount;
            _rBalance[to]   += rAmount;
        } else {
            _balance[from] -= tAmount;
            _balance[to]   += tAmount;
        }
        emit Transfer(from, to, tAmount);
    }

    function _burn(address from, uint256 tAmount) private {
        if (reflectionEnabled) {
            uint256 rAmount = _tokenToReflect(tAmount);
            _rBalance[from] -= rAmount;
            _rTotal         -= rAmount;
        } else {
            _balance[from] -= tAmount;
        }
        totalSupply -= tAmount;
        _tTotal     -= tAmount;
        emit Transfer(from, DEAD, tAmount);
        emit Burned(tAmount);
    }

    function _reflect(address from, uint256 tAmount) private {
        if (reflectionEnabled) {
            uint256 rAmount = _tokenToReflect(tAmount);
            _rBalance[from] -= rAmount;
            _rTotal         -= rAmount;   // reduces denominator → all holders get more
        } else {
            _balance[from] -= tAmount;
            // Without reflection enabled, treat as burn
            _tTotal -= tAmount;
        }
        emit Reflected(tAmount);
    }

    function _reflectToToken(uint256 rAmount) private view returns (uint256) {
        if (_rTotal == 0) return 0;
        return (rAmount * _tTotal) / _rTotal;
    }

    function _tokenToReflect(uint256 tAmount) private view returns (uint256) {
        if (_tTotal == 0) return 0;
        return (tAmount * _rTotal) / _tTotal;
    }

    // ─── Owner-only configuration ─────────────────────────────────────────

    modifier onlyOwner() {
        if (msg.sender != owner || ownershipRenounced) revert NotOwner();
        _;
    }

    function enableTrading() external onlyOwner {
        tradingEnabled = true;
        tradingBlock   = block.number;
        emit TradingEnabled(block.number);
    }

    function setTax(
        uint8 buyBurn, uint8 buyReflect, uint8 buyDev,
        uint8 sellBurn, uint8 sellReflect, uint8 sellDev
    ) external onlyOwner {
        require(buyBurn  + buyReflect  + buyDev  <= 25, "MemeToken: buy tax > 25%");
        require(sellBurn + sellReflect + sellDev <= 25, "MemeToken: sell tax > 25%");
        buyTax  = TaxConfig(buyBurn,  buyReflect,  buyDev);
        sellTax = TaxConfig(sellBurn, sellReflect, sellDev);
        emit TaxUpdated(buyBurn, buyReflect, buyDev, sellBurn, sellReflect, sellDev);
    }

    function setTransferTax(uint8 tBurn, uint8 tReflect, uint8 tDev) external onlyOwner {
        require(tBurn + tReflect + tDev <= 10, "MemeToken: transfer tax > 10%");
        transferTax = TaxConfig(tBurn, tReflect, tDev);
    }

    function setMaxWallet(uint256 bps) external onlyOwner {
        require(bps >= 100, "MemeToken: max wallet < 1%");  // min 1%
        maxWallet = (totalSupply * bps) / 10_000;
        emit MaxWalletUpdated(maxWallet);
    }

    function setMaxTx(uint256 bps) external onlyOwner {
        require(bps >= 50, "MemeToken: max TX < 0.5%");     // min 0.5%
        maxTx = (totalSupply * bps) / 10_000;
        emit MaxTxUpdated(maxTx);
    }

    function setBlacklist(address account, bool blacklisted_) external onlyOwner {
        isBlacklisted[account] = blacklisted_;
        emit BlacklistUpdated(account, blacklisted_);
    }

    function setTaxExempt(address account, bool exempt) external onlyOwner {
        isTaxExempt[account] = exempt;
        emit TaxExemptUpdated(account, exempt);
    }

    function setLiquidityPair(address pair, bool isPair_) external onlyOwner {
        isLiquidityPair[pair] = isPair_;
        emit LiquidityPairSet(pair, isPair_);
    }

    function setDevWallet(address dw) external onlyOwner {
        require(dw != address(0), "MemeToken: zero dev wallet");
        devWallet = dw;
    }

    /// @notice Permanently remove all owner controls.
    ///         Cannot be undone. Use to signal "rug proof" to community.
    function renounceOwnership() external onlyOwner {
        ownershipRenounced = true;
        emit OwnershipRenounced();
    }

    // ─── Manual burn ─────────────────────────────────────────────────────

    /// @notice Any holder can burn their own tokens.
    function burn(uint256 amount) external {
        _burn(msg.sender, amount);
    }
}
