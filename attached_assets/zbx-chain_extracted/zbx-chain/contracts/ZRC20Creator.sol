// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title ZRC20Creator — No-Code ZRC-20 Token Deployment Platform
/// @notice Allows anyone to deploy a fully featured ZRC-20 token with a single
///         transaction. No Solidity knowledge required.
///
/// @dev Deployed tokens are instances of `ZRC20Token` (inline below).
///      The factory tracks all deployments for the on-chain registry and
///      block explorer token discovery.
///
/// ## Features (user-selectable at deploy time)
///
/// - Name, symbol, decimals, initial supply
/// - Mintable (owner can mint new tokens)
/// - Burnable (holders can burn their own tokens)
/// - Pausable (owner can pause all transfers)
/// - Capped (max total supply enforced)
/// - Anti-bot launch protection (max tx amount for first N blocks)
///
/// ## Fee
///
/// A deployment fee of `deployFee` ZBX is collected by the factory owner
/// to sustain the no-code platform.

import { IERC20 } from "./interfaces/IERC20.sol";

// ─── Minimal ERC-20 implementation ────────────────────────────────────────────

contract ZRC20Token {
    string  public  name;
    string  public  symbol;
    uint8   public  decimals;
    uint256 public  totalSupply;
    uint256 public  maxSupply;     // 0 = uncapped
    address public  owner;
    bool    public  mintable;
    bool    public  burnable;
    bool    public  pausable;
    bool    public  paused;

    // Anti-bot: max transfer amount for first `antiBotBlocks` blocks.
    uint256 public  maxTxAmount;
    uint256 public  antiBotEndBlock;

    mapping(address => uint256)                     public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    event Transfer(address indexed from, address indexed to, uint256 amount);
    event Approval(address indexed owner_, address indexed spender, uint256 amount);
    event OwnershipTransferred(address indexed previousOwner, address indexed newOwner);
    event Paused(address account);
    event Unpaused(address account);

    error Unauthorized();
    error InsufficientBalance();
    error InsufficientAllowance();
    error CapExceeded();
    error ContractPaused();
    error AntiBotLimit();
    error NotMintable();
    error NotBurnable();
    error NotPausable();
    error ZeroAddress();

    modifier onlyOwner()  { if (msg.sender != owner) revert Unauthorized(); _; }
    modifier notPaused()  { if (paused) revert ContractPaused(); _; }

    constructor(
        string  memory _name,
        string  memory _symbol,
        uint8          _decimals,
        uint256        _initialSupply,
        uint256        _maxSupply,
        address        _owner,
        bool           _mintable,
        bool           _burnable,
        bool           _pausable,
        uint256        _maxTxAmount,
        uint256        _antiBotBlocks
    ) {
        if (_owner == address(0)) revert ZeroAddress();
        name      = _name;
        symbol    = _symbol;
        decimals  = _decimals;
        maxSupply = _maxSupply;
        owner     = _owner;
        mintable  = _mintable;
        burnable  = _burnable;
        pausable  = _pausable;
        maxTxAmount    = _maxTxAmount;
        antiBotEndBlock = block.number + _antiBotBlocks;

        if (_initialSupply > 0) {
            _mint(_owner, _initialSupply);
        }
    }

    // ── ERC-20 core ───────────────────────────────────────────────────────────

    function transfer(address to, uint256 amount) external notPaused returns (bool) {
        _checkAntiBot(amount);
        _transfer(msg.sender, to, amount);
        return true;
    }

    function transferFrom(address from, address to, uint256 amount)
        external notPaused returns (bool)
    {
        _checkAntiBot(amount);
        uint256 allowed = allowance[from][msg.sender];
        if (allowed < amount) revert InsufficientAllowance();
        if (allowed != type(uint256).max) {
            allowance[from][msg.sender] = allowed - amount;
        }
        _transfer(from, to, amount);
        return true;
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        emit Approval(msg.sender, spender, amount);
        return true;
    }

    // ── Mint / Burn ───────────────────────────────────────────────────────────

    function mint(address to, uint256 amount) external onlyOwner {
        if (!mintable) revert NotMintable();
        _mint(to, amount);
    }

    function burn(uint256 amount) external {
        if (!burnable) revert NotBurnable();
        _burn(msg.sender, amount);
    }

    function burnFrom(address from, uint256 amount) external {
        if (!burnable) revert NotBurnable();
        uint256 allowed = allowance[from][msg.sender];
        if (allowed < amount) revert InsufficientAllowance();
        if (allowed != type(uint256).max) {
            allowance[from][msg.sender] = allowed - amount;
        }
        _burn(from, amount);
    }

    // ── Pause ─────────────────────────────────────────────────────────────────

    function pause() external onlyOwner {
        if (!pausable) revert NotPausable();
        paused = true;
        emit Paused(msg.sender);
    }

    function unpause() external onlyOwner {
        if (!pausable) revert NotPausable();
        paused = false;
        emit Unpaused(msg.sender);
    }

    // ── Ownership ─────────────────────────────────────────────────────────────

    function transferOwnership(address newOwner) external onlyOwner {
        if (newOwner == address(0)) revert ZeroAddress();
        emit OwnershipTransferred(owner, newOwner);
        owner = newOwner;
    }

    function renounceOwnership() external onlyOwner {
        emit OwnershipTransferred(owner, address(0));
        owner = address(0);
    }

    // ── Internal ──────────────────────────────────────────────────────────────

    function _transfer(address from, address to, uint256 amount) internal {
        if (to == address(0)) revert ZeroAddress();
        if (balanceOf[from] < amount) revert InsufficientBalance();
        balanceOf[from] -= amount;
        balanceOf[to]   += amount;
        emit Transfer(from, to, amount);
    }

    function _mint(address to, uint256 amount) internal {
        if (maxSupply != 0 && totalSupply + amount > maxSupply) revert CapExceeded();
        totalSupply    += amount;
        balanceOf[to]  += amount;
        emit Transfer(address(0), to, amount);
    }

    function _burn(address from, uint256 amount) internal {
        if (balanceOf[from] < amount) revert InsufficientBalance();
        balanceOf[from] -= amount;
        totalSupply     -= amount;
        emit Transfer(from, address(0), amount);
    }

    function _checkAntiBot(uint256 amount) internal view {
        if (maxTxAmount != 0 && block.number <= antiBotEndBlock) {
            if (amount > maxTxAmount) revert AntiBotLimit();
        }
    }
}

// ─── Factory ──────────────────────────────────────────────────────────────────

contract ZRC20Creator {

    address public owner;
    uint256 public deployFee;  // in wei

    struct TokenInfo {
        address tokenAddress;
        address creator;
        string  name;
        string  symbol;
        uint256 deployedAt;
    }

    TokenInfo[] private _tokens;
    mapping(address => address[]) private _creatorTokens;

    event TokenDeployed(
        address indexed tokenAddress,
        address indexed creator,
        string  name,
        string  symbol,
        uint256 initialSupply
    );
    event FeeUpdated(uint256 newFee);
    event FeeWithdrawn(address indexed to, uint256 amount);

    error InsufficientFee(uint256 required, uint256 provided);
    error Unauthorized();
    error ZeroAddress();
    error WithdrawFailed();

    modifier onlyOwner() { if (msg.sender != owner) revert Unauthorized(); _; }

    constructor(uint256 _deployFee) {
        owner     = msg.sender;
        deployFee = _deployFee;
    }

    /// @notice Deploy a new ZRC-20 token.
    /// @param tokenName     Human-readable name (e.g. "My Token").
    /// @param tokenSymbol   Ticker symbol (e.g. "MTK").
    /// @param tokenDecimals Decimal places (18 recommended).
    /// @param initialSupply Tokens minted to `msg.sender` at deployment.
    /// @param maxSupply_    Maximum total supply (0 = uncapped).
    /// @param isMintable    Owner can call mint() after deployment.
    /// @param isBurnable    Holders can call burn() after deployment.
    /// @param isPausable    Owner can pause transfers.
    /// @param maxTxAmount_  Anti-bot max transfer; 0 = disabled.
    /// @param antiBotBlocks Number of blocks for anti-bot protection.
    function deployToken(
        string  calldata tokenName,
        string  calldata tokenSymbol,
        uint8            tokenDecimals,
        uint256          initialSupply,
        uint256          maxSupply_,
        bool             isMintable,
        bool             isBurnable,
        bool             isPausable,
        uint256          maxTxAmount_,
        uint256          antiBotBlocks
    ) external payable returns (address tokenAddress) {
        if (msg.value < deployFee) {
            revert InsufficientFee(deployFee, msg.value);
        }

        ZRC20Token token = new ZRC20Token(
            tokenName,
            tokenSymbol,
            tokenDecimals,
            initialSupply,
            maxSupply_,
            msg.sender,   // owner
            isMintable,
            isBurnable,
            isPausable,
            maxTxAmount_,
            antiBotBlocks
        );

        tokenAddress = address(token);

        _tokens.push(TokenInfo({
            tokenAddress: tokenAddress,
            creator:      msg.sender,
            name:         tokenName,
            symbol:       tokenSymbol,
            deployedAt:   block.number
        }));
        _creatorTokens[msg.sender].push(tokenAddress);

        emit TokenDeployed(tokenAddress, msg.sender, tokenName, tokenSymbol, initialSupply);
    }

    // ── View ──────────────────────────────────────────────────────────────────

    /// @notice Total number of tokens deployed through this factory.
    function tokenCount() external view returns (uint256) {
        return _tokens.length;
    }

    /// @notice Get token info by index.
    function getToken(uint256 index) external view returns (TokenInfo memory) {
        return _tokens[index];
    }

    /// @notice Get all tokens deployed by a specific creator.
    function getCreatorTokens(address creator) external view returns (address[] memory) {
        return _creatorTokens[creator];
    }

    // ── Admin ─────────────────────────────────────────────────────────────────

    function setDeployFee(uint256 newFee) external onlyOwner {
        deployFee = newFee;
        emit FeeUpdated(newFee);
    }

    function withdrawFees(address payable to) external onlyOwner {
        if (to == address(0)) revert ZeroAddress();
        uint256 bal = address(this).balance;
        (bool ok, ) = to.call{value: bal}("");
        if (!ok) revert WithdrawFailed();
        emit FeeWithdrawn(to, bal);
    }

    function transferOwnership(address newOwner) external onlyOwner {
        if (newOwner == address(0)) revert ZeroAddress();
        owner = newOwner;
    }
}
