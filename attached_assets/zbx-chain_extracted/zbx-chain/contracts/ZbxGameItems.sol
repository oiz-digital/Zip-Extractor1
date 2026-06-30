// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title  ZbxGameItems — ERC-1155 multi-token standard for game assets
/// @author Zebvix Technologies Pvt Ltd
///
/// @notice A fully-featured ERC-1155 implementation for in-game items:
///           - Weapons, armour, skins, consumables, power-ups, etc.
///           - Game developers define item types; authorised game contracts
///             mint and burn items as the game logic dictates.
///           - On-chain attributes (key → value) per item type.
///           - EIP-2981 royalty for secondary market sales.
///           - Soulbound flag per item type (non-transferable items).
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:module     Gaming / Items (ZEP-031)

interface IERC1155Receiver {
    function onERC1155Received(address, address, uint256, uint256, bytes calldata)
        external returns (bytes4);
    function onERC1155BatchReceived(address, address, uint256[] calldata, uint256[] calldata, bytes calldata)
        external returns (bytes4);
}

contract ZbxGameItems {

    // ─── Errors ───────────────────────────────────────────────────────────

    error NotOwner();
    error NotMinter();
    error TypeNotFound();
    error TypeExists();
    error MaxSupplyReached();
    error ZeroAddress();
    error InsufficientBalance();
    error LengthMismatch();
    error Soulbound(uint256 typeId);
    error NotApproved();
    error InvalidRoyalty();

    // ─── Events ───────────────────────────────────────────────────────────

    event TransferSingle(address indexed operator, address indexed from, address indexed to, uint256 id, uint256 value);
    event TransferBatch(address indexed operator, address indexed from, address indexed to, uint256[] ids, uint256[] values);
    event ApprovalForAll(address indexed owner, address indexed operator, bool approved);
    event URI(string value, uint256 indexed id);

    event ItemTypeCreated(uint256 indexed typeId, string name, uint256 maxSupply, bool soulbound);
    event AttributeSet(uint256 indexed typeId, string key, string value);
    event MinterSet(address indexed minter, bool enabled);

    // ─── Types ────────────────────────────────────────────────────────────

    struct ItemType {
        string  name;
        string  baseUri;
        uint256 maxSupply;
        uint256 totalMinted;
        bool    soulbound;       // cannot be transferred if true
        address royaltyRecipient;
        uint96  royaltyBps;      // basis points (0-1000 = 0-10%)
    }

    // ─── Storage ──────────────────────────────────────────────────────────

    address public owner;

    /// @notice typeId → ItemType definition
    mapping(uint256 => ItemType) public itemTypes;
    uint256 public nextTypeId;

    /// @notice owner → typeId → balance
    mapping(address => mapping(uint256 => uint256)) private _balances;

    /// @notice owner → operator → approved
    mapping(address => mapping(address => bool)) private _operatorApprovals;

    /// @notice typeId → attribute key → value (on-chain item stats)
    mapping(uint256 => mapping(string => string)) public attributes;

    /// @notice Addresses authorised to mint/burn items
    mapping(address => bool) public isMinter;

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor() {
        owner = msg.sender;
    }

    // ─── ERC-1155 core ───────────────────────────────────────────────────

    function balanceOf(address account, uint256 id) public view returns (uint256) {
        require(account != address(0), "GameItems: zero address");
        return _balances[account][id];
    }

    function balanceOfBatch(address[] calldata accounts, uint256[] calldata ids)
        external view returns (uint256[] memory balances)
    {
        if (accounts.length != ids.length) revert LengthMismatch();
        balances = new uint256[](accounts.length);
        for (uint256 i; i < accounts.length; ++i) {
            balances[i] = balanceOf(accounts[i], ids[i]);
        }
    }

    function setApprovalForAll(address operator, bool approved) external {
        require(operator != msg.sender, "GameItems: self approval");
        _operatorApprovals[msg.sender][operator] = approved;
        emit ApprovalForAll(msg.sender, operator, approved);
    }

    function isApprovedForAll(address account, address operator) public view returns (bool) {
        return _operatorApprovals[account][operator];
    }

    function safeTransferFrom(
        address from,
        address to,
        uint256 id,
        uint256 amount,
        bytes calldata data
    ) external {
        if (to == address(0)) revert ZeroAddress();
        if (from != msg.sender && !isApprovedForAll(from, msg.sender)) revert NotApproved();
        if (itemTypes[id].soulbound && from != address(0)) revert Soulbound(id);

        uint256 bal = _balances[from][id];
        if (bal < amount) revert InsufficientBalance();
        unchecked { _balances[from][id] = bal - amount; }
        _balances[to][id] += amount;

        emit TransferSingle(msg.sender, from, to, id, amount);
        _checkReceiver(msg.sender, from, to, id, amount, data);
    }

    function safeBatchTransferFrom(
        address from,
        address to,
        uint256[] calldata ids,
        uint256[] calldata amounts,
        bytes calldata data
    ) external {
        if (to == address(0)) revert ZeroAddress();
        if (ids.length != amounts.length) revert LengthMismatch();
        if (from != msg.sender && !isApprovedForAll(from, msg.sender)) revert NotApproved();

        for (uint256 i; i < ids.length; ++i) {
            uint256 id = ids[i];
            if (itemTypes[id].soulbound && from != address(0)) revert Soulbound(id);
            uint256 bal = _balances[from][id];
            if (bal < amounts[i]) revert InsufficientBalance();
            unchecked { _balances[from][id] = bal - amounts[i]; }
            _balances[to][id] += amounts[i];
        }

        emit TransferBatch(msg.sender, from, to, ids, amounts);
        _checkBatchReceiver(msg.sender, from, to, ids, amounts, data);
    }

    // ─── Item type management ─────────────────────────────────────────────

    /// @notice Register a new item type.  Only callable by owner.
    /// @param  name             Human-readable name (e.g. "Fire Sword").
    /// @param  baseUri          Metadata URI (IPFS or on-chain).
    /// @param  maxSupply        Maximum items that can ever be minted (0 = unlimited).
    /// @param  soulbound        If true, items of this type cannot be transferred.
    /// @param  royaltyRecipient Address receiving secondary-sale royalties.
    /// @param  royaltyBps       Royalty in basis points (max 1000 = 10%).
    /// @return typeId           The new item type ID.
    function createItemType(
        string calldata name,
        string calldata baseUri,
        uint256         maxSupply,
        bool            soulbound,
        address         royaltyRecipient,
        uint96          royaltyBps
    ) external returns (uint256 typeId) {
        if (msg.sender != owner)  revert NotOwner();
        if (royaltyBps > 1000)   revert InvalidRoyalty();

        typeId = ++nextTypeId;
        itemTypes[typeId] = ItemType({
            name:             name,
            baseUri:          baseUri,
            maxSupply:        maxSupply,
            totalMinted:      0,
            soulbound:        soulbound,
            royaltyRecipient: royaltyRecipient,
            royaltyBps:       royaltyBps
        });

        emit ItemTypeCreated(typeId, name, maxSupply, soulbound);
        emit URI(baseUri, typeId);
    }

    /// @notice Set an on-chain attribute for an item type (e.g. "damage" → "150").
    function setAttribute(uint256 typeId, string calldata key, string calldata value) external {
        if (msg.sender != owner) revert NotOwner();
        if (itemTypes[typeId].maxSupply == 0 && itemTypes[typeId].totalMinted == 0
            && bytes(itemTypes[typeId].name).length == 0) revert TypeNotFound();
        attributes[typeId][key] = value;
        emit AttributeSet(typeId, key, value);
    }

    // ─── Mint / Burn ──────────────────────────────────────────────────────

    /// @notice Mint `amount` items of `typeId` to `to`.
    ///         Only callable by authorised minters (game contracts).
    function mint(address to, uint256 typeId, uint256 amount, bytes calldata data) external {
        if (!isMinter[msg.sender]) revert NotMinter();
        if (to == address(0))      revert ZeroAddress();
        if (amount == 0) return;

        ItemType storage t = itemTypes[typeId];
        if (bytes(t.name).length == 0) revert TypeNotFound();
        if (t.maxSupply > 0 && t.totalMinted + amount > t.maxSupply)
            revert MaxSupplyReached();

        t.totalMinted += amount;
        _balances[to][typeId] += amount;

        emit TransferSingle(msg.sender, address(0), to, typeId, amount);
        _checkReceiver(msg.sender, address(0), to, typeId, amount, data);
    }

    /// @notice Batch mint multiple item types.
    function mintBatch(
        address            to,
        uint256[] calldata typeIds,
        uint256[] calldata amounts,
        bytes     calldata data
    ) external {
        if (!isMinter[msg.sender]) revert NotMinter();
        if (to == address(0))      revert ZeroAddress();
        if (typeIds.length != amounts.length) revert LengthMismatch();

        for (uint256 i; i < typeIds.length; ++i) {
            uint256 tid = typeIds[i];
            ItemType storage t = itemTypes[tid];
            if (bytes(t.name).length == 0) revert TypeNotFound();
            if (t.maxSupply > 0 && t.totalMinted + amounts[i] > t.maxSupply)
                revert MaxSupplyReached();
            t.totalMinted  += amounts[i];
            _balances[to][tid] += amounts[i];
        }

        emit TransferBatch(msg.sender, address(0), to, typeIds, amounts);
        _checkBatchReceiver(msg.sender, address(0), to, typeIds, amounts, data);
    }

    /// @notice Burn items (game contract removes consumed/used items).
    function burn(address from, uint256 typeId, uint256 amount) external {
        if (!isMinter[msg.sender] && msg.sender != from) revert NotMinter();
        uint256 bal = _balances[from][typeId];
        if (bal < amount) revert InsufficientBalance();
        unchecked { _balances[from][typeId] = bal - amount; }
        emit TransferSingle(msg.sender, from, address(0), typeId, amount);
    }

    // ─── EIP-2981 Royalties ───────────────────────────────────────────────

    function royaltyInfo(uint256 typeId, uint256 salePrice)
        external view returns (address receiver, uint256 royaltyAmount)
    {
        ItemType storage t = itemTypes[typeId];
        receiver      = t.royaltyRecipient;
        royaltyAmount = (salePrice * t.royaltyBps) / 10_000;
    }

    // ─── EIP-165 ─────────────────────────────────────────────────────────

    function supportsInterface(bytes4 interfaceId) external pure returns (bool) {
        return interfaceId == 0xd9b67a26  // ERC-1155
            || interfaceId == 0x2a55205a  // EIP-2981 royalties
            || interfaceId == 0x01ffc9a7; // EIP-165
    }

    function uri(uint256 typeId) external view returns (string memory) {
        return itemTypes[typeId].baseUri;
    }

    // ─── Admin ────────────────────────────────────────────────────────────

    function setMinter(address minter, bool enabled) external {
        if (msg.sender != owner) revert NotOwner();
        isMinter[minter] = enabled;
        emit MinterSet(minter, enabled);
    }

    function transferOwnership(address newOwner) external {
        if (msg.sender != owner) revert NotOwner();
        if (newOwner == address(0)) revert ZeroAddress();
        owner = newOwner;
    }

    // ─── Internal ─────────────────────────────────────────────────────────

    function _checkReceiver(
        address operator, address from, address to,
        uint256 id, uint256 amount, bytes memory data
    ) private {
        if (to.code.length > 0) {
            bytes4 ret = IERC1155Receiver(to)
                .onERC1155Received(operator, from, id, amount, data);
            require(ret == IERC1155Receiver.onERC1155Received.selector,
                    "GameItems: bad receiver");
        }
    }

    function _checkBatchReceiver(
        address operator, address from, address to,
        uint256[] memory ids, uint256[] memory amounts, bytes memory data
    ) private {
        if (to.code.length > 0) {
            bytes4 ret = IERC1155Receiver(to)
                .onERC1155BatchReceived(operator, from, ids, amounts, data);
            require(ret == IERC1155Receiver.onERC1155BatchReceived.selector,
                    "GameItems: bad batch receiver");
        }
    }
}
