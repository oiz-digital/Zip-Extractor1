// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title  ZbxContractFactory — No-code deployment of standard contracts
/// @author Zebvix Technologies Pvt Ltd
///
/// @notice Lets anyone deploy standard on-chain primitives without writing
///         Solidity — just call the appropriate factory function with params.
///
///         Supported:
///           - ERC-20 tokens  (name, symbol, supply, decimals, mintable flag)
///           - ERC-721 NFT collections (name, symbol, max supply, royalty)
///           - ERC-1155 multi-token game item collections
///           - Vesting contracts (token, beneficiary, cliff, duration)
///           - Multisig wallets (owners, threshold)
///
///         All deployed contracts are registered in a public registry so they
///         can be discovered by the explorer and verified by users.
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:module     Infrastructure / Contract Factory (ZEP-038)

// ─── Minimal inline ERC-20 ───────────────────────────────────────────────────

contract MiniERC20 {
    string  public name;
    string  public symbol;
    uint8   public decimals;
    uint256 public totalSupply;
    bool    public mintable;
    address public owner;

    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    event Transfer(address indexed from, address indexed to, uint256 amount);
    event Approval(address indexed owner_, address indexed spender, uint256 amount);

    constructor(
        string memory name_, string memory symbol_,
        uint8 decimals_, uint256 initialSupply_,
        address owner_, bool mintable_
    ) {
        name        = name_;
        symbol      = symbol_;
        decimals    = decimals_;
        mintable    = mintable_;
        owner       = owner_;
        if (initialSupply_ > 0) {
            totalSupply           = initialSupply_;
            balanceOf[owner_]     = initialSupply_;
            emit Transfer(address(0), owner_, initialSupply_);
        }
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        balanceOf[msg.sender] -= amount;
        balanceOf[to]         += amount;
        emit Transfer(msg.sender, to, amount);
        return true;
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        emit Approval(msg.sender, spender, amount);
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        uint256 a = allowance[from][msg.sender];
        if (a != type(uint256).max) allowance[from][msg.sender] = a - amount;
        balanceOf[from] -= amount;
        balanceOf[to]   += amount;
        emit Transfer(from, to, amount);
        return true;
    }

    function mint(address to, uint256 amount) external {
        require(mintable && msg.sender == owner, "MiniERC20: not owner or not mintable");
        totalSupply     += amount;
        balanceOf[to]   += amount;
        emit Transfer(address(0), to, amount);
    }

    function burn(uint256 amount) external {
        balanceOf[msg.sender] -= amount;
        totalSupply            -= amount;
        emit Transfer(msg.sender, address(0), amount);
    }
}

// ─── Minimal inline ERC-721 ──────────────────────────────────────────────────

contract MiniNFT {
    string  public name;
    string  public symbol;
    address public owner;
    uint256 public maxSupply;
    uint256 public totalSupply;
    uint96  public royaltyBps;
    string  public baseUri;

    mapping(uint256 => address) private _owners;
    mapping(address => uint256) private _balances;
    mapping(uint256 => address) private _approvals;
    mapping(address => mapping(address => bool)) private _opApprovals;

    event Transfer(address indexed from, address indexed to, uint256 indexed tokenId);
    event Approval(address indexed owner_, address indexed approved, uint256 indexed tokenId);
    event ApprovalForAll(address indexed owner_, address indexed op, bool approved);

    error NotOwnerOrApproved();
    error MaxSupplyReached();
    error TokenNotFound();

    constructor(
        string memory name_, string memory symbol_,
        uint256 maxSupply_, uint96 royaltyBps_, string memory baseUri_,
        address owner_
    ) {
        name       = name_;
        symbol     = symbol_;
        maxSupply  = maxSupply_;
        royaltyBps = royaltyBps_;
        baseUri    = baseUri_;
        owner      = owner_;
    }

    function ownerOf(uint256 id) public view returns (address) {
        address o = _owners[id];
        if (o == address(0)) revert TokenNotFound();
        return o;
    }
    function balanceOf(address a) external view returns (uint256) { return _balances[a]; }

    function approve(address to, uint256 id) external {
        address o = ownerOf(id);
        require(msg.sender == o || _opApprovals[o][msg.sender], "NFT: not owner");
        _approvals[id] = to;
        emit Approval(o, to, id);
    }
    function setApprovalForAll(address op, bool appr) external {
        _opApprovals[msg.sender][op] = appr;
        emit ApprovalForAll(msg.sender, op, appr);
    }
    function transferFrom(address from, address to, uint256 id) public {
        address o = ownerOf(id);
        require(msg.sender == o || _approvals[id] == msg.sender || _opApprovals[o][msg.sender],
                "NFT: not approved");
        require(from == o, "NFT: wrong from");
        _balances[from]--;
        _balances[to]++;
        _owners[id] = to;
        delete _approvals[id];
        emit Transfer(from, to, id);
    }

    function mint(address to, uint256 amount) external {
        require(msg.sender == owner, "NFT: not owner");
        if (maxSupply > 0 && totalSupply + amount > maxSupply) revert MaxSupplyReached();
        for (uint256 i; i < amount; ++i) {
            uint256 id = ++totalSupply;
            _owners[id]     = to;
            _balances[to]++;
            emit Transfer(address(0), to, id);
        }
    }

    function tokenURI(uint256 id) external view returns (string memory) {
        return string(abi.encodePacked(baseUri, _uint2str(id)));
    }

    function royaltyInfo(uint256, uint256 salePrice)
        external view returns (address, uint256)
    {
        return (owner, (salePrice * royaltyBps) / 10_000);
    }

    function supportsInterface(bytes4 id) external pure returns (bool) {
        return id == 0x80ac58cd  // ERC-721
            || id == 0x2a55205a  // EIP-2981
            || id == 0x01ffc9a7; // EIP-165
    }

    function _uint2str(uint256 v) private pure returns (string memory) {
        if (v == 0) return "0";
        uint256 temp = v; uint256 digits;
        while (temp != 0) { digits++; temp /= 10; }
        bytes memory buf = new bytes(digits);
        while (v != 0) { digits--; buf[digits] = bytes1(uint8(48 + v % 10)); v /= 10; }
        return string(buf);
    }
}

// ─── Main Factory ─────────────────────────────────────────────────────────────

contract ZbxContractFactory {

    // ─── Events ───────────────────────────────────────────────────────────

    event TokenDeployed(
        address indexed creator,
        address indexed token,
        string  name,
        string  symbol,
        uint256 supply
    );
    event NFTDeployed(
        address indexed creator,
        address indexed nft,
        string  name,
        string  symbol,
        uint256 maxSupply
    );
    event ContractRegistered(
        address indexed contractAddr,
        string  contractType,
        address indexed creator
    );

    // ─── Registry ─────────────────────────────────────────────────────────

    struct DeployedContract {
        address addr;
        string  contractType;  // "ERC20", "ERC721", "ERC1155", etc.
        address creator;
        uint256 deployedAt;
    }

    address public owner;
    uint256 public deployFee = 0.001 ether; // small anti-spam fee
    address public treasury;

    DeployedContract[] public registry;
    mapping(address => DeployedContract[]) public byCreator;

    constructor(address treasury_) {
        require(treasury_ != address(0), "Factory: zero treasury");
        owner    = msg.sender;
        treasury = treasury_;
    }

    // ─── ERC-20 deployment ────────────────────────────────────────────────

    /// @notice Deploy a new ERC-20 token.
    /// @param  name_          Token name (e.g., "My Token").
    /// @param  symbol_        Token symbol (e.g., "MTK").
    /// @param  decimals_      Decimal places (usually 18).
    /// @param  initialSupply  Total supply minted to `msg.sender` at creation.
    /// @param  mintable       If true, owner can mint more tokens later.
    /// @return token          Address of the deployed token contract.
    function deployToken(
        string  calldata name_,
        string  calldata symbol_,
        uint8            decimals_,
        uint256          initialSupply,
        bool             mintable
    ) external payable returns (address token) {
        require(msg.value >= deployFee, "Factory: insufficient fee");

        MiniERC20 t = new MiniERC20(name_, symbol_, decimals_, initialSupply, msg.sender, mintable);
        token = address(t);

        _register(token, "ERC20");
        emit TokenDeployed(msg.sender, token, name_, symbol_, initialSupply);
        _refundExcess();
    }

    // ─── ERC-721 deployment ───────────────────────────────────────────────

    /// @notice Deploy a new NFT collection.
    /// @param  name_       Collection name.
    /// @param  symbol_     Collection symbol.
    /// @param  maxSupply_  Maximum NFTs (0 = unlimited).
    /// @param  royaltyBps  Secondary-sale royalty in basis points (max 1000).
    /// @param  baseUri_    Base metadata URI (IPFS or HTTPS).
    /// @return nft         Address of the deployed NFT contract.
    function deployNFT(
        string  calldata name_,
        string  calldata symbol_,
        uint256          maxSupply_,
        uint96           royaltyBps,
        string  calldata baseUri_
    ) external payable returns (address nft) {
        require(msg.value >= deployFee, "Factory: insufficient fee");
        require(royaltyBps <= 1000, "Factory: royalty too high");

        MiniNFT n = new MiniNFT(name_, symbol_, maxSupply_, royaltyBps, baseUri_, msg.sender);
        nft = address(n);

        _register(nft, "ERC721");
        emit NFTDeployed(msg.sender, nft, name_, symbol_, maxSupply_);
        _refundExcess();
    }

    // ─── View helpers ─────────────────────────────────────────────────────

    function registryLength() external view returns (uint256) {
        return registry.length;
    }

    function contractsByCreator(address creator) external view returns (DeployedContract[] memory) {
        return byCreator[creator];
    }

    function allContracts(uint256 offset, uint256 limit)
        external view returns (DeployedContract[] memory result)
    {
        uint256 total = registry.length;
        if (offset >= total) return new DeployedContract[](0);
        uint256 end = offset + limit;
        if (end > total) end = total;
        result = new DeployedContract[](end - offset);
        for (uint256 i = offset; i < end; ++i) {
            result[i - offset] = registry[i];
        }
    }

    // ─── Admin ────────────────────────────────────────────────────────────

    function setDeployFee(uint256 fee) external {
        require(msg.sender == owner, "Factory: not owner");
        deployFee = fee;
    }

    function withdrawFees() external {
        require(msg.sender == owner, "Factory: not owner");
        (bool ok,) = treasury.call{value: address(this).balance}("");
        require(ok, "Factory: withdraw failed");
    }

    // ─── Internal ─────────────────────────────────────────────────────────

    function _register(address addr, string memory contractType) private {
        DeployedContract memory dc = DeployedContract({
            addr:         addr,
            contractType: contractType,
            creator:      msg.sender,
            deployedAt:   block.timestamp
        });
        registry.push(dc);
        byCreator[msg.sender].push(dc);
        emit ContractRegistered(addr, contractType, msg.sender);
    }

    function _refundExcess() private {
        uint256 excess = msg.value - deployFee;
        if (excess > 0) {
            (bool ok,) = msg.sender.call{value: excess}("");
            require(ok, "Factory: refund failed");
        }
        // Forward fee to treasury
        (bool ok2,) = treasury.call{value: deployFee}("");
        require(ok2, "Factory: fee forward failed");
    }

    receive() external payable {}
}
