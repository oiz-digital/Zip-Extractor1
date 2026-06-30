// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title ZRC721Creator — No-Code NFT Collection Deployment Platform
/// @notice Deploy a full-featured ZRC-721 NFT collection without writing Solidity.
///
/// ## Features
/// - Custom name, symbol, max supply, mint price
/// - IPFS base URI for metadata
/// - Allowlist / whitelist minting phase
/// - Royalty support (ERC-2981)
/// - Reveal mechanism (placeholder URI before reveal)
/// - Owner can withdraw mint proceeds

contract ZRC721Token {
    string  public name;
    string  public symbol;
    uint256 public maxSupply;
    uint256 public mintPrice;   // wei per token
    uint256 public totalMinted;
    address public owner;
    bool    public revealed;
    bool    public paused;

    // Royalties (ERC-2981)
    address public royaltyReceiver;
    uint96  public royaltyBps;   // basis points (0–10000)

    string private _baseURI;
    string private _hiddenURI;

    mapping(uint256 => address) private _ownerOf;
    mapping(address => uint256) private _balanceOf;
    mapping(uint256 => address) private _approvals;
    mapping(address => mapping(address => bool)) private _operatorApprovals;
    mapping(address => bool)  public  allowlist;
    bool public allowlistActive;

    event Transfer(address indexed from, address indexed to, uint256 indexed tokenId);
    event Approval(address indexed owner_, address indexed approved, uint256 indexed tokenId);
    event ApprovalForAll(address indexed owner_, address indexed operator, bool approved);

    error Unauthorized();
    error SoldOut();
    error ContractPaused();
    error InsufficientPayment();
    error NotAllowlisted();
    error ZeroAddress();
    error TokenNotFound();
    error NotOwnerOrApproved();

    modifier onlyOwner()  { if (msg.sender != owner) revert Unauthorized(); _; }
    modifier notPaused()  { if (paused) revert ContractPaused(); _; }

    constructor(
        string  memory _name,
        string  memory _symbol,
        uint256        _maxSupply,
        uint256        _mintPrice,
        string  memory _baseURI_,
        string  memory _hiddenURI_,
        address        _royaltyReceiver,
        uint96         _royaltyBps,
        address        _owner
    ) {
        if (_owner == address(0)) revert ZeroAddress();
        name             = _name;
        symbol           = _symbol;
        maxSupply        = _maxSupply;
        mintPrice        = _mintPrice;
        _baseURI         = _baseURI_;
        _hiddenURI       = _hiddenURI_;
        royaltyReceiver  = _royaltyReceiver;
        royaltyBps       = _royaltyBps;
        owner            = _owner;
    }

    // ── Mint ──────────────────────────────────────────────────────────────────

    /// @notice Mint `quantity` NFTs to `msg.sender`.
    function mint(uint256 quantity) external payable notPaused {
        if (allowlistActive && !allowlist[msg.sender]) revert NotAllowlisted();
        if (totalMinted + quantity > maxSupply) revert SoldOut();
        if (msg.value < mintPrice * quantity) revert InsufficientPayment();

        for (uint256 i = 0; i < quantity; i++) {
            uint256 tokenId = ++totalMinted;
            _ownerOf[tokenId]           = msg.sender;
            _balanceOf[msg.sender]      += 1;
            emit Transfer(address(0), msg.sender, tokenId);
        }
    }

    /// @notice Owner mint (airdrop / reserve) — no payment required.
    function ownerMint(address to, uint256 quantity) external onlyOwner {
        if (to == address(0)) revert ZeroAddress();
        if (totalMinted + quantity > maxSupply) revert SoldOut();
        for (uint256 i = 0; i < quantity; i++) {
            uint256 tokenId = ++totalMinted;
            _ownerOf[tokenId]   = to;
            _balanceOf[to]     += 1;
            emit Transfer(address(0), to, tokenId);
        }
    }

    // ── ERC-721 core ──────────────────────────────────────────────────────────

    function ownerOf(uint256 tokenId) public view returns (address) {
        address o = _ownerOf[tokenId];
        if (o == address(0)) revert TokenNotFound();
        return o;
    }

    function balanceOf(address account) external view returns (uint256) {
        return _balanceOf[account];
    }

    function tokenURI(uint256 tokenId) external view returns (string memory) {
        if (_ownerOf[tokenId] == address(0)) revert TokenNotFound();
        if (!revealed) return _hiddenURI;
        return string(abi.encodePacked(_baseURI, _toString(tokenId), ".json"));
    }

    function approve(address to, uint256 tokenId) external {
        address tokenOwner = ownerOf(tokenId);
        if (msg.sender != tokenOwner && !_operatorApprovals[tokenOwner][msg.sender])
            revert Unauthorized();
        _approvals[tokenId] = to;
        emit Approval(tokenOwner, to, tokenId);
    }

    function getApproved(uint256 tokenId) external view returns (address) {
        if (_ownerOf[tokenId] == address(0)) revert TokenNotFound();
        return _approvals[tokenId];
    }

    function setApprovalForAll(address operator, bool approved) external {
        _operatorApprovals[msg.sender][operator] = approved;
        emit ApprovalForAll(msg.sender, operator, approved);
    }

    function isApprovedForAll(address owner_, address operator) external view returns (bool) {
        return _operatorApprovals[owner_][operator];
    }

    function transferFrom(address from, address to, uint256 tokenId) public {
        if (ownerOf(tokenId) != from) revert Unauthorized();
        if (
            msg.sender != from
            && msg.sender != _approvals[tokenId]
            && !_operatorApprovals[from][msg.sender]
        ) revert NotOwnerOrApproved();
        if (to == address(0)) revert ZeroAddress();
        delete _approvals[tokenId];
        _balanceOf[from]  -= 1;
        _balanceOf[to]    += 1;
        _ownerOf[tokenId]  = to;
        emit Transfer(from, to, tokenId);
    }

    function safeTransferFrom(address from, address to, uint256 tokenId) external {
        transferFrom(from, to, tokenId);
    }

    function safeTransferFrom(address from, address to, uint256 tokenId, bytes calldata) external {
        transferFrom(from, to, tokenId);
    }

    // ── ERC-2981 Royalties ────────────────────────────────────────────────────

    function royaltyInfo(uint256, uint256 salePrice)
        external view returns (address receiver, uint256 royaltyAmount)
    {
        receiver      = royaltyReceiver;
        royaltyAmount = (salePrice * royaltyBps) / 10_000;
    }

    // ── Admin ─────────────────────────────────────────────────────────────────

    function reveal(string calldata newBaseURI) external onlyOwner {
        _baseURI = newBaseURI;
        revealed = true;
    }

    function setBaseURI(string calldata newBaseURI) external onlyOwner {
        _baseURI = newBaseURI;
    }

    function setAllowlist(address[] calldata addresses, bool status) external onlyOwner {
        for (uint256 i = 0; i < addresses.length; i++) {
            allowlist[addresses[i]] = status;
        }
    }

    function setAllowlistActive(bool active) external onlyOwner {
        allowlistActive = active;
    }

    function setPaused(bool _paused) external onlyOwner { paused = _paused; }

    function setMintPrice(uint256 newPrice) external onlyOwner { mintPrice = newPrice; }

    function setRoyalty(address receiver, uint96 bps) external onlyOwner {
        royaltyReceiver = receiver;
        royaltyBps      = bps;
    }

    function withdraw(address payable to) external onlyOwner {
        (bool ok,) = to.call{value: address(this).balance}("");
        require(ok, "withdraw failed");
    }

    function transferOwnership(address newOwner) external onlyOwner {
        if (newOwner == address(0)) revert ZeroAddress();
        owner = newOwner;
    }

    function _toString(uint256 value) internal pure returns (string memory) {
        if (value == 0) return "0";
        uint256 temp = value;
        uint256 digits;
        while (temp != 0) { digits++; temp /= 10; }
        bytes memory buffer = new bytes(digits);
        while (value != 0) {
            digits -= 1;
            buffer[digits] = bytes1(uint8(48 + uint256(value % 10)));
            value /= 10;
        }
        return string(buffer);
    }
}

// ─── Factory ──────────────────────────────────────────────────────────────────

contract ZRC721Creator {
    address public owner;
    uint256 public deployFee;

    struct CollectionInfo {
        address tokenAddress;
        address creator;
        string  name;
        string  symbol;
        uint256 maxSupply;
        uint256 deployedAt;
    }

    CollectionInfo[] private _collections;
    mapping(address => address[]) private _creatorCollections;

    event CollectionDeployed(
        address indexed tokenAddress,
        address indexed creator,
        string  name,
        string  symbol,
        uint256 maxSupply
    );

    error InsufficientFee(uint256 required, uint256 provided);
    error Unauthorized();
    error ZeroAddress();

    modifier onlyOwner() { if (msg.sender != owner) revert Unauthorized(); _; }

    constructor(uint256 _deployFee) {
        owner     = msg.sender;
        deployFee = _deployFee;
    }

    /// @notice Deploy a new NFT collection.
    function deployCollection(
        string  calldata collectionName,
        string  calldata collectionSymbol,
        uint256          maxSupply_,
        uint256          mintPrice_,
        string  calldata baseURI,
        string  calldata hiddenURI,
        address          royaltyReceiver,
        uint96           royaltyBps
    ) external payable returns (address collectionAddress) {
        if (msg.value < deployFee) revert InsufficientFee(deployFee, msg.value);

        ZRC721Token token = new ZRC721Token(
            collectionName, collectionSymbol,
            maxSupply_, mintPrice_,
            baseURI, hiddenURI,
            royaltyReceiver, royaltyBps,
            msg.sender
        );

        collectionAddress = address(token);
        _collections.push(CollectionInfo({
            tokenAddress: collectionAddress,
            creator:      msg.sender,
            name:         collectionName,
            symbol:       collectionSymbol,
            maxSupply:    maxSupply_,
            deployedAt:   block.number
        }));
        _creatorCollections[msg.sender].push(collectionAddress);

        emit CollectionDeployed(collectionAddress, msg.sender, collectionName, collectionSymbol, maxSupply_);
    }

    function collectionCount() external view returns (uint256) { return _collections.length; }
    function getCollection(uint256 index) external view returns (CollectionInfo memory) { return _collections[index]; }
    function getCreatorCollections(address creator) external view returns (address[] memory) { return _creatorCollections[creator]; }

    function setDeployFee(uint256 newFee) external onlyOwner { deployFee = newFee; }
    function withdrawFees(address payable to) external onlyOwner {
        if (to == address(0)) revert ZeroAddress();
        (bool ok,) = to.call{value: address(this).balance}("");
        require(ok, "withdraw failed");
    }
    function transferOwnership(address newOwner) external onlyOwner {
        if (newOwner == address(0)) revert ZeroAddress();
        owner = newOwner;
    }
}
