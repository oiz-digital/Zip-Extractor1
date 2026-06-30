// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { IZRC721 } from "./interfaces/IZRC721.sol";

/// @dev ERC-721 receiver hook — every contract that wants to be a transfer
///      target MUST return this magic selector from `onERC721Received`. EOAs
///      are exempt and treated as always-receivers.
interface IERC721Receiver {
    function onERC721Received(
        address operator,
        address from,
        uint256 tokenId,
        bytes calldata data
    ) external returns (bytes4);
}

/// @title ZRC721Base — Reference implementation of ZRC-721 NFT standard.
/// @notice Deploy (or inherit) to create an NFT collection on Zebvix Chain.
///         Includes: minting, enumeration, metadata, royalties, batch mint.
///
/// @dev Gas optimisations:
///        - Sequential token IDs starting from 1 (no gap minting).
///        - Batch mint writes `quantity` tokens in O(quantity) storage ops.
///        - Owner enumeration uses two mappings (O(1) lookup, O(1) removal).
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:standard   ZRC-721 v1.0

abstract contract ZRC721Base is IZRC721 {

    // ─── Metadata ─────────────────────────────────────────────────────────

    string private _name;
    string private _symbol;
    string private _baseURI;     // base for tokenURI (e.g. "ipfs://QmColl.../")
    uint256 private _nextTokenId = 1;

    // ─── Ownership ────────────────────────────────────────────────────────

    mapping(uint256 => address) private _owners;
    mapping(address => uint256) private _balances;
    mapping(uint256 => address) private _tokenApprovals;
    mapping(address => mapping(address => bool)) private _operatorApprovals;

    // ─── Enumerable ───────────────────────────────────────────────────────

    uint256[] private _allTokens;
    mapping(uint256 => uint256) private _allTokensIndex;         // tokenId → index
    mapping(address => uint256[]) private _ownedTokens;
    mapping(uint256 => uint256) private _ownedTokensIndex;       // tokenId → owner-index

    // ─── Royalties (EIP-2981) ─────────────────────────────────────────────

    address  private _royaltyReceiver;
    uint96   private _royaltyBps;    // basis points (100 = 1%)

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor(
        string memory name_,
        string memory symbol_,
        string memory baseURI_
    ) {
        _name    = name_;
        _symbol  = symbol_;
        _baseURI = baseURI_;
    }

    // ─── IZRC721 Core ─────────────────────────────────────────────────────

    function name()   public view virtual override returns (string memory) { return _name; }
    function symbol() public view virtual override returns (string memory) { return _symbol; }

    function tokenURI(uint256 tokenId) public view virtual override returns (string memory) {
        require(_exists(tokenId), "ZRC721: nonexistent token");
        return string(abi.encodePacked(_baseURI, _toString(tokenId)));
    }

    function balanceOf(address owner) public view virtual override returns (uint256) {
        require(owner != address(0), "ZRC721: balance of zero address");
        return _balances[owner];
    }

    function ownerOf(uint256 tokenId) public view virtual override returns (address) {
        address owner = _owners[tokenId];
        require(owner != address(0), "ZRC721: nonexistent token");
        return owner;
    }

    function approve(address to, uint256 tokenId) public virtual override {
        address owner = ownerOf(tokenId);
        require(to != owner,                                    "ZRC721: approve to current owner");
        require(msg.sender == owner || isApprovedForAll(owner, msg.sender), "ZRC721: not owner/operator");
        _tokenApprovals[tokenId] = to;
        emit Approval(owner, to, tokenId);
    }

    function setApprovalForAll(address operator, bool approved) public virtual override {
        require(operator != msg.sender, "ZRC721: approve to caller");
        _operatorApprovals[msg.sender][operator] = approved;
        emit ApprovalForAll(msg.sender, operator, approved);
    }

    function getApproved(uint256 tokenId) public view virtual override returns (address) {
        require(_exists(tokenId), "ZRC721: nonexistent token");
        return _tokenApprovals[tokenId];
    }

    function isApprovedForAll(address owner, address operator) public view virtual override returns (bool) {
        return _operatorApprovals[owner][operator];
    }

    function transferFrom(address from, address to, uint256 tokenId) public virtual override {
        require(_isApprovedOrOwner(msg.sender, tokenId), "ZRC721: not approved");
        _transfer(from, to, tokenId);
    }

    function safeTransferFrom(address from, address to, uint256 tokenId) public virtual override {
        safeTransferFrom(from, to, tokenId, "");
    }

    function safeTransferFrom(address from, address to, uint256 tokenId, bytes calldata data) public virtual override {
        require(_isApprovedOrOwner(msg.sender, tokenId), "ZRC721: not approved");
        _transfer(from, to, tokenId);
        require(_checkOnERC721Received(from, to, tokenId, data), "ZRC721: non-receiver");
    }

    // ─── Enumerable ───────────────────────────────────────────────────────

    function totalSupply() public view virtual override returns (uint256) { return _allTokens.length; }

    function tokenByIndex(uint256 index) public view virtual override returns (uint256) {
        require(index < _allTokens.length, "ZRC721: global index out of bounds");
        return _allTokens[index];
    }

    function tokenOfOwnerByIndex(address owner, uint256 index) public view virtual override returns (uint256) {
        require(index < _balances[owner], "ZRC721: owner index out of bounds");
        return _ownedTokens[owner][index];
    }

    // ─── ZRC-721 Extension: Batch Mint ───────────────────────────────────

    function batchMint(address to, uint256 quantity) external virtual override returns (uint256[] memory ids) {
        require(quantity > 0 && quantity <= 200, "ZRC721: invalid quantity (max 200)");
        ids = new uint256[](quantity);
        for (uint256 i; i < quantity; ++i) {
            ids[i] = _mint(to);
        }
    }

    // ─── ZRC-721 Extension: EIP-2981 Royalties ───────────────────────────

    function royaltyInfo(uint256, uint256 salePrice)
        public view virtual override returns (address, uint256)
    {
        return (_royaltyReceiver, (salePrice * _royaltyBps) / 10_000);
    }

    function setDefaultRoyalty(address receiver, uint96 bps) public virtual override {
        require(bps <= 1000, "ZRC721: royalty > 10%");
        _royaltyReceiver = receiver;
        _royaltyBps      = bps;
    }

    // ─── Internal Mint / Burn ─────────────────────────────────────────────

    function _mint(address to) internal virtual returns (uint256 tokenId) {
        require(to != address(0), "ZRC721: mint to zero address");
        tokenId = _nextTokenId++;

        _balances[to]++;
        _owners[tokenId] = to;

        _allTokensIndex[tokenId] = _allTokens.length;
        _allTokens.push(tokenId);

        _ownedTokensIndex[tokenId] = _ownedTokens[to].length;
        _ownedTokens[to].push(tokenId);

        emit Transfer(address(0), to, tokenId);
    }

    function _burn(uint256 tokenId) internal virtual {
        address owner = ownerOf(tokenId);
        delete _tokenApprovals[tokenId];
        _balances[owner]--;
        delete _owners[tokenId];

        // Remove from _allTokens
        uint256 lastIdx = _allTokens.length - 1;
        uint256 idx     = _allTokensIndex[tokenId];
        uint256 lastToken = _allTokens[lastIdx];
        _allTokens[idx] = lastToken;
        _allTokensIndex[lastToken] = idx;
        _allTokens.pop();
        delete _allTokensIndex[tokenId];

        // Remove from _ownedTokens
        uint256 lastOwnerIdx = _ownedTokens[owner].length - 1;
        uint256 ownedIdx     = _ownedTokensIndex[tokenId];
        uint256 lastOwnedToken = _ownedTokens[owner][lastOwnerIdx];
        _ownedTokens[owner][ownedIdx] = lastOwnedToken;
        _ownedTokensIndex[lastOwnedToken] = ownedIdx;
        _ownedTokens[owner].pop();
        delete _ownedTokensIndex[tokenId];

        emit Transfer(owner, address(0), tokenId);
    }

    function _transfer(address from, address to, uint256 tokenId) internal virtual {
        require(ownerOf(tokenId) == from, "ZRC721: transfer from wrong owner");
        require(to != address(0),         "ZRC721: transfer to zero address");
        delete _tokenApprovals[tokenId];

        _balances[from]--;
        _balances[to]++;
        _owners[tokenId] = to;

        // Update enumeration
        uint256 fromIdx = _ownedTokensIndex[tokenId];
        uint256 lastIdx = _ownedTokens[from].length - 1;
        if (fromIdx != lastIdx) {
            uint256 last = _ownedTokens[from][lastIdx];
            _ownedTokens[from][fromIdx] = last;
            _ownedTokensIndex[last] = fromIdx;
        }
        _ownedTokens[from].pop();
        _ownedTokensIndex[tokenId] = _ownedTokens[to].length;
        _ownedTokens[to].push(tokenId);

        emit Transfer(from, to, tokenId);
    }

    // ─── Helpers ──────────────────────────────────────────────────────────

    function _exists(uint256 tokenId) internal view returns (bool) {
        return _owners[tokenId] != address(0);
    }

    function _isApprovedOrOwner(address spender, uint256 tokenId) internal view returns (bool) {
        address owner = ownerOf(tokenId);
        return spender == owner
            || isApprovedForAll(owner, spender)
            || getApproved(tokenId) == spender;
    }

    /// @custom:audit-2026-04-30  S4-A3 (HIGH) closed.
    /// Per EIP-721, when `to` is a contract it MUST return the magic selector
    /// `IERC721Receiver.onERC721Received.selector` or the transfer reverts.
    /// EOAs (no code) are treated as always-receivers. The previous stub
    /// `return true` allowed locked-up NFTs in any non-receiver contract.
    function _checkOnERC721Received(
        address from,
        address to,
        uint256 tokenId,
        bytes memory data
    ) private returns (bool) {
        if (to.code.length == 0) {
            return true; // EOA — always accepts.
        }
        try IERC721Receiver(to).onERC721Received(msg.sender, from, tokenId, data)
            returns (bytes4 retval)
        {
            return retval == IERC721Receiver.onERC721Received.selector;
        } catch {
            return false;
        }
    }

    function _toString(uint256 v) internal pure returns (string memory) {
        if (v == 0) return "0";
        uint256 tmp = v; uint256 len;
        while (tmp > 0) { len++; tmp /= 10; }
        bytes memory buf = new bytes(len);
        while (v > 0) { buf[--len] = bytes1(uint8(48 + v % 10)); v /= 10; }
        return string(buf);
    }

    // ─── EIP-165 supportsInterface (S21) ───────────────────────────────────
    //
    // Base claim for the ZRC-721 NFT standard. Concrete subclasses (e.g. a
    // collection contract that adds royalties or extensions) should
    // `override` this and OR-in their extra interfaceIds via `super.`.
    //
    // Note: ZbxNFT (the chain's native NFT) does NOT inherit ZRC721Base —
    // it ships its own self-contained ERC-721 + ERC-2981 impl with its
    // own supportsInterface. So this base method exists purely for future
    // ZRC721Base-derived collections and for the SupportsInterface test
    // suite (via TestableZRC721Base).
    function supportsInterface(bytes4 interfaceId) public pure virtual returns (bool) {
        return interfaceId == type(IZRC721).interfaceId
            || interfaceId == 0x01ffc9a7;   // EIP-165 itself
    }
}