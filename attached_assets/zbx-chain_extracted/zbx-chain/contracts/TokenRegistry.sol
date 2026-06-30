// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title TokenRegistry — Global on-chain registry of all ZRC-20 tokens.
/// @notice Every ZRC-20 token deployed via ZRC20Factory is automatically
///         registered here. Tokens can also self-register or be registered
///         by governance.
///
/// @dev   This is the canonical source of truth for "what tokens exist on
///        Zebvix Chain". Wallets, explorers, and DEXes read from here.
///        Reading is free and fully public. Registration is permissioned.
///
/// @custom:zbx-chain  Chain ID 8989

contract TokenRegistry {

    // ─── Token Entry ──────────────────────────────────────────────────────

    enum TokenCategory {
        NATIVE,     // ZBX itself
        WRAPPED,    // WZBX
        BRIDGED,    // ZbxETH, ZbxBTC, ZbxUSDT, etc.
        STABLECOIN, // USD-pegged
        GOVERNANCE, // ZBXGov, DAO tokens
        UTILITY,    // fee tokens, platform tokens
        MEME,       // community tokens
        UNKNOWN
    }

    struct TokenInfo {
        address token;           // ZRC-20 contract address
        string  name;
        string  symbol;
        uint8   decimals;
        address creator;         // who registered it
        uint256 registeredAt;    // block timestamp
        TokenCategory category;
        bool    verified;        // verified by Zebvix team
        bool    bridged;         // is this a bridged asset?
        string  logoURI;         // IPFS / HTTPS logo
        string  website;         // project website
        string  coingeckoId;     // CoinGecko ID (if listed)
    }

    // ─── State ────────────────────────────────────────────────────────────

    address public owner;
    address public factory;      // ZRC20Factory — auto-registers on deploy

    address[]                          public allTokens;
    mapping(address => TokenInfo)      public tokenInfo;
    mapping(address => bool)           public isRegistered;
    mapping(TokenCategory => address[]) public tokensByCategory;
    mapping(string  => address)        public tokenBySymbol;   // symbol → address
    mapping(address => bool)           public registrars;      // allowed to register

    // ─── Events ───────────────────────────────────────────────────────────

    event TokenRegistered(
        address indexed token,
        address indexed creator,
        string  name,
        string  symbol,
        TokenCategory category
    );
    event TokenVerified(address indexed token, bool verified);
    event TokenInfoUpdated(address indexed token);
    event RegistrarUpdated(address indexed registrar, bool allowed);
    event FactoryUpdated(address indexed prev, address indexed next);

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor(address factory_) {
        owner   = msg.sender;
        factory = factory_;
        registrars[msg.sender]  = true;
        registrars[factory_]    = true;
    }

    modifier onlyOwner()     { require(msg.sender == owner,          "Registry: not owner");     _; }
    modifier onlyRegistrar() { require(registrars[msg.sender],       "Registry: not registrar"); _; }

    // ─── Register ─────────────────────────────────────────────────────────

    /// @notice Register a new ZRC-20 token.
    /// @dev    Called by ZRC20Factory automatically, or by verified registrars.
    function register(
        address       token,
        string  calldata name,
        string  calldata symbol,
        uint8            decimals,
        address          creator,
        TokenCategory    category,
        bool             bridged,
        string  calldata logoURI,
        string  calldata website,
        string  calldata coingeckoId
    ) external onlyRegistrar {
        require(token != address(0),       "Registry: zero address");
        require(!isRegistered[token],      "Registry: already registered");
        require(bytes(name).length   > 0,  "Registry: empty name");
        require(bytes(symbol).length > 0,  "Registry: empty symbol");

        // Symbol collision check — warn if symbol taken but do not revert
        // (symbols are not unique on-chain; explorer shows address as tiebreaker)

        tokenInfo[token] = TokenInfo({
            token:          token,
            name:           name,
            symbol:         symbol,
            decimals:       decimals,
            creator:        creator,
            registeredAt:   block.timestamp,
            category:       category,
            verified:       false,
            bridged:        bridged,
            logoURI:        logoURI,
            website:        website,
            coingeckoId:    coingeckoId
        });

        isRegistered[token] = true;
        allTokens.push(token);
        tokensByCategory[category].push(token);
        if (tokenBySymbol[symbol] == address(0)) {
            tokenBySymbol[symbol] = token;
        }

        emit TokenRegistered(token, creator, name, symbol, category);
    }

    // ─── Verify ───────────────────────────────────────────────────────────

    /// @notice Mark a token as verified (Zebvix team audit seal).
    function setVerified(address token, bool verified) external onlyOwner {
        require(isRegistered[token], "Registry: not registered");
        tokenInfo[token].verified = verified;
        emit TokenVerified(token, verified);
    }

    // ─── Update metadata ──────────────────────────────────────────────────

    /// @notice Update mutable metadata (logo, website, coingecko).
    ///         Only the token creator or owner can update.
    function updateMetadata(
        address token,
        string calldata logoURI,
        string calldata website,
        string calldata coingeckoId
    ) external {
        require(isRegistered[token], "Registry: not registered");
        require(
            msg.sender == tokenInfo[token].creator || msg.sender == owner,
            "Registry: not creator"
        );
        tokenInfo[token].logoURI     = logoURI;
        tokenInfo[token].website     = website;
        tokenInfo[token].coingeckoId = coingeckoId;
        emit TokenInfoUpdated(token);
    }

    // ─── Admin ────────────────────────────────────────────────────────────

    function setRegistrar(address registrar, bool allowed) external onlyOwner {
        registrars[registrar] = allowed;
        emit RegistrarUpdated(registrar, allowed);
    }

    function setFactory(address factory_) external onlyOwner {
        emit FactoryUpdated(factory, factory_);
        registrars[factory] = false;
        factory = factory_;
        registrars[factory_] = true;
    }

    function transferOwnership(address to) external onlyOwner {
        owner = to;
    }

    // ─── Views ────────────────────────────────────────────────────────────

    function totalTokens() external view returns (uint256) {
        return allTokens.length;
    }

    function tokensInCategory(TokenCategory cat) external view returns (address[] memory) {
        return tokensByCategory[cat];
    }

    /// @notice Paginated token list for explorer / wallet UI.
    /// @param  offset  Starting index.
    /// @param  limit   Max results (capped at 100).
    function getTokens(uint256 offset, uint256 limit)
        external view returns (TokenInfo[] memory result)
    {
        uint256 total = allTokens.length;
        if (offset >= total) return result;

        uint256 end = offset + limit;
        if (end > total) end = total;
        if (end - offset > 100) end = offset + 100;

        result = new TokenInfo[](end - offset);
        for (uint256 i = offset; i < end; ++i) {
            result[i - offset] = tokenInfo[allTokens[i]];
        }
    }

    /// @notice Get full info for a list of token addresses (batch read).
    function getBatch(address[] calldata tokens)
        external view returns (TokenInfo[] memory result)
    {
        result = new TokenInfo[](tokens.length);
        for (uint256 i; i < tokens.length; ++i) {
            result[i] = tokenInfo[tokens[i]];
        }
    }

    /// @notice Check if token is verified.
    function isVerified(address token) external view returns (bool) {
        return tokenInfo[token].verified;
    }
}