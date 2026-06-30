// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

/// @title ZbxNFT — Native NFT Standard for ZBX chain (ZEP-006)
/// @notice ERC-721 compatible + ZBX-native extensions:
///         - On-chain royalties (ERC-2981)
///         - Soulbound tokens (non-transferable)
///         - Lazy minting (off-chain signature, on-chain reveal)
///         - Pay ID integration (mint with human-readable name)
/// @dev    Chainlink-compatible: ZbxNFT implements ERC-165 supportsInterface

interface IERC165 {
    function supportsInterface(bytes4) external view returns (bool);
}

interface IERC721 {
    event Transfer(address indexed from, address indexed to, uint256 indexed tokenId);
    event Approval(address indexed owner, address indexed approved, uint256 indexed tokenId);
    event ApprovalForAll(address indexed owner, address indexed operator, bool approved);

    function balanceOf(address owner)                          external view returns (uint256);
    function ownerOf(uint256 tokenId)                          external view returns (address);
    function safeTransferFrom(address from, address to, uint256 tokenId, bytes calldata data) external;
    function safeTransferFrom(address from, address to, uint256 tokenId) external;
    function transferFrom(address from, address to, uint256 tokenId) external;
    function approve(address to, uint256 tokenId)              external;
    function setApprovalForAll(address operator, bool approved) external;
    function getApproved(uint256 tokenId)                      external view returns (address);
    function isApprovedForAll(address owner, address operator) external view returns (bool);
}

/// @notice ERC-2981 Royalties interface
interface IERC2981 {
    function royaltyInfo(uint256 tokenId, uint256 salePrice)
        external view returns (address receiver, uint256 royaltyAmount);
}

/// @notice ERC-721 acceptance hook — destination contracts must return the
/// 4-byte magic value or `safeTransferFrom` reverts. Without this check the
/// "safe" variant is identical to plain transferFrom and tokens can be sent
/// into contracts that have no way to handle them. See AUDIT_2026-04-30.md M-14.
interface IERC721Receiver {
    function onERC721Received(
        address operator, address from, uint256 tokenId, bytes calldata data
    ) external returns (bytes4);
}

contract ZbxNFT is IERC721, IERC2981, IERC165 {

    // ── State ──────────────────────────────────────────────────────────────

    string  public name;
    string  public symbol;
    address public owner;

    mapping(uint256 => address) private _owners;
    mapping(address => uint256) private _balances;
    mapping(uint256 => address) private _tokenApprovals;
    mapping(address => mapping(address => bool)) private _operatorApprovals;
    mapping(uint256 => string)  private _tokenURIs;

    uint256 private _totalSupply;
    uint256 public  nextTokenId;

    // Royalties (ERC-2981): creator earns % on secondary sales
    address public royaltyRecipient;
    uint256 public royaltyBps;         // basis points (e.g. 500 = 5%)

    // Soulbound: token IDs that are non-transferable
    mapping(uint256 => bool) public isSoulbound;

    // Lazy mint: off-chain signed voucher → on-chain reveal
    mapping(bytes32 => bool) public voucherUsed;

    // ── Events ─────────────────────────────────────────────────────────────

    event Minted(address indexed to, uint256 indexed tokenId, string uri);
    event LazyMinted(address indexed to, uint256 indexed tokenId, bytes32 voucherHash);
    event RoyaltyUpdated(address recipient, uint256 bps);
    event SoulboundSet(uint256 indexed tokenId, bool soulbound);

    // ── Errors ─────────────────────────────────────────────────────────────

    error NotOwner();
    error NotApproved();
    error SoulboundTransferDenied(uint256 tokenId);
    error VoucherAlreadyUsed();
    error InvalidVoucher();
    error ZeroAddress();
    error TokenNotFound(uint256 tokenId);

    // ── Constructor ────────────────────────────────────────────────────────

    constructor(
        string memory _name,
        string memory _symbol,
        address       _royaltyRecipient,
        uint256       _royaltyBps
    ) {
        name             = _name;
        symbol           = _symbol;
        owner            = msg.sender;
        royaltyRecipient = _royaltyRecipient;
        royaltyBps       = _royaltyBps;
    }

    // ── ERC-721 core ───────────────────────────────────────────────────────

    function balanceOf(address _owner) public view override returns (uint256) {
        if (_owner == address(0)) revert ZeroAddress();
        return _balances[_owner];
    }

    function ownerOf(uint256 tokenId) public view override returns (address) {
        address tokenOwner = _owners[tokenId];
        if (tokenOwner == address(0)) revert TokenNotFound(tokenId);
        return tokenOwner;
    }

    function approve(address to, uint256 tokenId) external override {
        address tokenOwner = ownerOf(tokenId);
        if (msg.sender != tokenOwner && !isApprovedForAll(tokenOwner, msg.sender))
            revert NotApproved();
        _tokenApprovals[tokenId] = to;
        emit Approval(tokenOwner, to, tokenId);
    }

    function getApproved(uint256 tokenId) public view override returns (address) {
        return _tokenApprovals[tokenId];
    }

    function setApprovalForAll(address operator, bool approved) external override {
        _operatorApprovals[msg.sender][operator] = approved;
        emit ApprovalForAll(msg.sender, operator, approved);
    }

    function isApprovedForAll(address _owner, address operator) public view override returns (bool) {
        return _operatorApprovals[_owner][operator];
    }

    function transferFrom(address from, address to, uint256 tokenId) public override {
        if (isSoulbound[tokenId]) revert SoulboundTransferDenied(tokenId);
        if (!_isApprovedOrOwner(msg.sender, tokenId)) revert NotApproved();
        _transfer(from, to, tokenId);
    }

    function safeTransferFrom(address from, address to, uint256 tokenId) external override {
        transferFrom(from, to, tokenId);
        _checkOnERC721Received(from, to, tokenId, "");
    }

    function safeTransferFrom(address from, address to, uint256 tokenId, bytes calldata data) external override {
        transferFrom(from, to, tokenId);
        _checkOnERC721Received(from, to, tokenId, data);
    }

    /// Per EIP-721, if `to` is a contract it MUST return the magic selector
    /// `IERC721Receiver.onERC721Received.selector`. EOAs are skipped.
    function _checkOnERC721Received(
        address from, address to, uint256 tokenId, bytes memory data
    ) internal {
        uint256 size;
        // S25-Y3 assembly: EIP-721 §3.2 contract-detection — `extcodesize(to)`
        // returns 0 for EOAs (skip onERC721Received check). Solidity has no
        // direct equivalent except `to.code.length` which adds a memory copy
        // of the entire bytecode; assembly is the canonical OZ pattern here.
        assembly { size := extcodesize(to) }
        if (size == 0) return; // EOA — nothing to do
        try IERC721Receiver(to).onERC721Received(msg.sender, from, tokenId, data)
            returns (bytes4 retval) {
            require(
                retval == IERC721Receiver.onERC721Received.selector,
                "ZbxNFT: receiver rejected"
            );
        } catch {
            revert("ZbxNFT: non-ERC721Receiver");
        }
    }

    // ── Minting ────────────────────────────────────────────────────────────

    /// @notice Mint a new NFT (owner only).
    function mint(address to, string calldata uri) external returns (uint256 tokenId) {
        if (msg.sender != owner) revert NotOwner();
        tokenId = nextTokenId++;
        _mint(to, tokenId, uri);
    }

    /// @notice Lazy mint — user presents a signed voucher to claim NFT.
    /// @param  voucher  Signed by the contract owner off-chain.
    function lazyMint(
        address       to,
        string calldata uri,
        uint256       price,
        bytes calldata signature
    ) external payable returns (uint256 tokenId) {
        if (msg.value < price) revert InvalidVoucher();

        // Compute and verify voucher
        bytes32 voucherHash = keccak256(abi.encodePacked(to, uri, price, address(this)));
        if (voucherUsed[voucherHash]) revert VoucherAlreadyUsed();
        if (!_verifySignature(voucherHash, signature, owner)) revert InvalidVoucher();

        voucherUsed[voucherHash] = true;
        tokenId = nextTokenId++;
        _mint(to, tokenId, uri);
        emit LazyMinted(to, tokenId, voucherHash);
    }

    /// @notice Mark a token as soulbound (non-transferable).
    function setSoulbound(uint256 tokenId, bool bound) external {
        if (msg.sender != owner) revert NotOwner();
        isSoulbound[tokenId] = bound;
        emit SoulboundSet(tokenId, bound);
    }

    // ── ERC-2981 Royalties ─────────────────────────────────────────────────

    function royaltyInfo(uint256, uint256 salePrice)
        external view override
        returns (address receiver, uint256 royaltyAmount)
    {
        return (royaltyRecipient, salePrice * royaltyBps / 10_000);
    }

    function setRoyalty(address recipient, uint256 bps) external {
        if (msg.sender != owner) revert NotOwner();
        royaltyRecipient = recipient;
        royaltyBps       = bps;
        emit RoyaltyUpdated(recipient, bps);
    }

    // ── ERC-165 ────────────────────────────────────────────────────────────

    function supportsInterface(bytes4 interfaceId) external pure override returns (bool) {
        return interfaceId == type(IERC721).interfaceId   // 0x80ac58cd
            || interfaceId == type(IERC2981).interfaceId  // 0x2a55205a
            || interfaceId == type(IERC165).interfaceId;  // 0x01ffc9a7
    }

    // ── Internal ───────────────────────────────────────────────────────────

    function _mint(address to, uint256 tokenId, string memory uri) internal {
        if (to == address(0)) revert ZeroAddress();
        _owners[tokenId] = to;
        _balances[to]++;
        _tokenURIs[tokenId] = uri;
        _totalSupply++;
        emit Transfer(address(0), to, tokenId);
        emit Minted(to, tokenId, uri);
    }

    function _transfer(address from, address to, uint256 tokenId) internal {
        if (to == address(0)) revert ZeroAddress();
        delete _tokenApprovals[tokenId];
        _balances[from]--;
        _balances[to]++;
        _owners[tokenId] = to;
        emit Transfer(from, to, tokenId);
    }

    function _isApprovedOrOwner(address spender, uint256 tokenId) internal view returns (bool) {
        address tokenOwner = _owners[tokenId];
        return spender == tokenOwner
            || getApproved(tokenId) == spender
            || isApprovedForAll(tokenOwner, spender);
    }

    /// secp256k1 curve order n / 2 — for EIP-2 low-S enforcement.
    /// Without this, an attacker who has any valid voucher signature can
    /// trivially derive a second valid signature with `s' = n - s` and `v` flipped,
    /// yielding a *different* `voucherHash`-tracking key. Combined with the way
    /// `voucherUsed` is keyed on `voucherHash` (not the signature), this would
    /// be cosmetic here, but enforcing low-S is a free defense-in-depth that
    /// matches every other ECDSA recovery path in the codebase.
    /// See AUDIT_2026-04-30.md M-14.
    uint256 internal constant _HALF_N =
        0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0;

    function _verifySignature(bytes32 hash, bytes memory sig, address expected) internal pure returns (bool) {
        if (sig.length != 65) return false;
        bytes32 r; bytes32 s; uint8 v;
        // S25-Y3 assembly: split fixed-65-byte ECDSA signature into (r,s,v) from
        // memory bytes. `mload(add(sig,32))` skips the 32-byte length prefix.
        // Bounds proven by `sig.length != 65 → return` on the previous line.
        // EIP-2 low-S enforcement happens immediately below.
        assembly { r := mload(add(sig,32)); s := mload(add(sig,64)); v := byte(0,mload(add(sig,96))) }
        if (uint256(s) > _HALF_N)        return false; // EIP-2: reject high-S
        if (v != 27 && v != 28)          return false;
        // EIP-191 personal_sign — must be one byte 0x19 then literal text +
        // actual newline + length "32". The previous double-escaped form
        // ("\\x19...\\n32") stored a 5-char string starting with `\` so every
        // signature recover returned a wrong address. Use bytes1(0x19) and "\n"
        // so wallet-produced sigs validate.
        bytes32 ethHash = keccak256(
            abi.encodePacked(bytes1(0x19), "Ethereum Signed Message:\n32", hash)
        );
        address rec = ecrecover(ethHash, v, r, s);
        return rec != address(0) && rec == expected;
    }

    function tokenURI(uint256 tokenId) external view returns (string memory) {
        if (_owners[tokenId] == address(0)) revert TokenNotFound(tokenId);
        return _tokenURIs[tokenId];
    }

    function totalSupply() external view returns (uint256) { return _totalSupply; }
}