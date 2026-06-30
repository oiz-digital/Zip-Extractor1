// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title ZRC1155Creator — No-Code Multi-Token (ERC-1155) Collection Platform
/// @notice Deploy ERC-1155 multi-token contracts without writing Solidity.
///         Supports fungible tokens, NFTs, and game items in a single contract.

contract ZRC1155Token {
    address public owner;
    string  public name;
    string  public symbol;
    bool    public paused;

    // ERC-2981 royalties
    address public royaltyReceiver;
    uint96  public royaltyBps;

    mapping(uint256 => mapping(address => uint256)) public balanceOf;
    mapping(address => mapping(address => bool))    public isApprovedForAll;
    mapping(uint256 => uint256)                     public totalSupply;
    mapping(uint256 => uint256)                     public maxSupply;   // 0 = unlimited
    mapping(uint256 => uint256)                     public mintPrice;
    mapping(uint256 => string)                      private _tokenURIs;

    string private _baseURI;

    event TransferSingle(address indexed operator, address indexed from, address indexed to, uint256 id, uint256 amount);
    event TransferBatch(address indexed operator, address indexed from, address indexed to, uint256[] ids, uint256[] amounts);
    event ApprovalForAll(address indexed account, address indexed operator, bool approved);
    event URI(string value, uint256 indexed id);
    event TokenTypeCreated(uint256 indexed id, uint256 maxSupply_, uint256 mintPrice_);

    error Unauthorized();
    error ContractPaused();
    error CapExceeded(uint256 id);
    error InsufficientBalance(uint256 id);
    error InsufficientPayment();
    error LengthMismatch();
    error ZeroAddress();

    modifier onlyOwner() { if (msg.sender != owner) revert Unauthorized(); _; }
    modifier notPaused() { if (paused) revert ContractPaused(); _; }

    constructor(
        string  memory _name,
        string  memory _symbol,
        string  memory baseURI_,
        address        _royaltyReceiver,
        uint96         _royaltyBps,
        address        _owner
    ) {
        if (_owner == address(0)) revert ZeroAddress();
        name             = _name;
        symbol           = _symbol;
        _baseURI         = baseURI_;
        royaltyReceiver  = _royaltyReceiver;
        royaltyBps       = _royaltyBps;
        owner            = _owner;
    }

    // ── Token type management ─────────────────────────────────────────────────

    /// @notice Owner creates a new token type (id must not already exist).
    function createTokenType(
        uint256 id,
        uint256 maxSupply_,
        uint256 mintPrice_,
        string  calldata tokenURI_
    ) external onlyOwner {
        require(totalSupply[id] == 0 && maxSupply[id] == 0, "type exists");
        maxSupply[id]  = maxSupply_;
        mintPrice[id]  = mintPrice_;
        if (bytes(tokenURI_).length > 0) {
            _tokenURIs[id] = tokenURI_;
            emit URI(tokenURI_, id);
        }
        emit TokenTypeCreated(id, maxSupply_, mintPrice_);
    }

    // ── Minting ───────────────────────────────────────────────────────────────

    function mint(address to, uint256 id, uint256 amount) external payable notPaused {
        if (to == address(0)) revert ZeroAddress();
        if (msg.value < mintPrice[id] * amount) revert InsufficientPayment();
        if (maxSupply[id] != 0 && totalSupply[id] + amount > maxSupply[id])
            revert CapExceeded(id);

        totalSupply[id]     += amount;
        balanceOf[id][to]   += amount;
        emit TransferSingle(msg.sender, address(0), to, id, amount);
    }

    function ownerMint(address to, uint256 id, uint256 amount) external onlyOwner {
        if (to == address(0)) revert ZeroAddress();
        if (maxSupply[id] != 0 && totalSupply[id] + amount > maxSupply[id])
            revert CapExceeded(id);

        totalSupply[id]     += amount;
        balanceOf[id][to]   += amount;
        emit TransferSingle(msg.sender, address(0), to, id, amount);
    }

    // ── ERC-1155 core ─────────────────────────────────────────────────────────

    function safeTransferFrom(
        address from, address to, uint256 id, uint256 amount, bytes calldata
    ) external notPaused {
        if (from != msg.sender && !isApprovedForAll[from][msg.sender]) revert Unauthorized();
        if (to == address(0)) revert ZeroAddress();
        if (balanceOf[id][from] < amount) revert InsufficientBalance(id);
        balanceOf[id][from] -= amount;
        balanceOf[id][to]   += amount;
        emit TransferSingle(msg.sender, from, to, id, amount);
    }

    function safeBatchTransferFrom(
        address from, address to,
        uint256[] calldata ids, uint256[] calldata amounts, bytes calldata
    ) external notPaused {
        if (ids.length != amounts.length) revert LengthMismatch();
        if (from != msg.sender && !isApprovedForAll[from][msg.sender]) revert Unauthorized();
        if (to == address(0)) revert ZeroAddress();

        for (uint256 i = 0; i < ids.length; i++) {
            uint256 id = ids[i]; uint256 amount = amounts[i];
            if (balanceOf[id][from] < amount) revert InsufficientBalance(id);
            balanceOf[id][from] -= amount;
            balanceOf[id][to]   += amount;
        }
        emit TransferBatch(msg.sender, from, to, ids, amounts);
    }

    function setApprovalForAll(address operator, bool approved) external {
        isApprovedForAll[msg.sender][operator] = approved;
        emit ApprovalForAll(msg.sender, operator, approved);
    }

    function balanceOfBatch(
        address[] calldata accounts, uint256[] calldata ids
    ) external view returns (uint256[] memory bals) {
        if (accounts.length != ids.length) revert LengthMismatch();
        bals = new uint256[](accounts.length);
        for (uint256 i = 0; i < accounts.length; i++) {
            bals[i] = balanceOf[ids[i]][accounts[i]];
        }
    }

    function uri(uint256 id) external view returns (string memory) {
        string memory tokenUri = _tokenURIs[id];
        if (bytes(tokenUri).length > 0) return tokenUri;
        return _baseURI;
    }

    // ── ERC-2981 ──────────────────────────────────────────────────────────────

    function royaltyInfo(uint256, uint256 salePrice)
        external view returns (address receiver, uint256 royaltyAmount)
    {
        receiver      = royaltyReceiver;
        royaltyAmount = (salePrice * royaltyBps) / 10_000;
    }

    // ── Admin ─────────────────────────────────────────────────────────────────

    function setPaused(bool _paused) external onlyOwner { paused = _paused; }
    function setBaseURI(string calldata baseURI_) external onlyOwner { _baseURI = baseURI_; }
    function setRoyalty(address receiver, uint96 bps) external onlyOwner {
        royaltyReceiver = receiver; royaltyBps = bps;
    }
    function withdraw(address payable to) external onlyOwner {
        (bool ok,) = to.call{value: address(this).balance}(""); require(ok, "withdraw failed");
    }
    function transferOwnership(address newOwner) external onlyOwner {
        if (newOwner == address(0)) revert ZeroAddress();
        owner = newOwner;
    }
}

// ─── Factory ──────────────────────────────────────────────────────────────────

contract ZRC1155Creator {
    address public owner;
    uint256 public deployFee;

    struct ContractInfo {
        address contractAddress;
        address creator;
        string  name;
        uint256 deployedAt;
    }

    ContractInfo[] private _contracts;
    mapping(address => address[]) private _creatorContracts;

    event ContractDeployed(address indexed contractAddress, address indexed creator, string name);

    error InsufficientFee(uint256 required, uint256 provided);
    error Unauthorized();
    error ZeroAddress();

    modifier onlyOwner() { if (msg.sender != owner) revert Unauthorized(); _; }

    constructor(uint256 _deployFee) { owner = msg.sender; deployFee = _deployFee; }

    function deployContract(
        string  calldata contractName,
        string  calldata contractSymbol,
        string  calldata baseURI,
        address          royaltyReceiver,
        uint96           royaltyBps
    ) external payable returns (address contractAddress) {
        if (msg.value < deployFee) revert InsufficientFee(deployFee, msg.value);

        ZRC1155Token token = new ZRC1155Token(
            contractName, contractSymbol, baseURI,
            royaltyReceiver, royaltyBps, msg.sender
        );

        contractAddress = address(token);
        _contracts.push(ContractInfo({
            contractAddress: contractAddress,
            creator:         msg.sender,
            name:            contractName,
            deployedAt:      block.number
        }));
        _creatorContracts[msg.sender].push(contractAddress);

        emit ContractDeployed(contractAddress, msg.sender, contractName);
    }

    function contractCount() external view returns (uint256) { return _contracts.length; }
    function getContract(uint256 index) external view returns (ContractInfo memory) { return _contracts[index]; }
    function getCreatorContracts(address creator) external view returns (address[] memory) { return _creatorContracts[creator]; }

    function setDeployFee(uint256 newFee) external onlyOwner { deployFee = newFee; }
    function withdrawFees(address payable to) external onlyOwner {
        if (to == address(0)) revert ZeroAddress();
        (bool ok,) = to.call{value: address(this).balance}(""); require(ok, "withdraw failed");
    }
    function transferOwnership(address newOwner) external onlyOwner {
        if (newOwner == address(0)) revert ZeroAddress();
        owner = newOwner;
    }
}
