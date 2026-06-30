// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { ReentrancyGuard } from "./libraries/ReentrancyGuard.sol";

/// @title  ZbxNftMarketplace — minimal escrow-less ERC-721 marketplace
/// @author Zebvix Technologies Pvt Ltd
/// @notice Ships the spec-aligned settlement protections from day one
///         (SEC-2026-05-09 Pass-19 Tier-2 #6):
///           * Fee-on-settle (no governance bypass — fee is hard-coded
///             at construction, immutable).
///           * EIP-2981 royalty honoring on every sale.
///           * Signature-bound listings with explicit expiry +
///             cancel-by-nonce.
///           * `nonReentrant` on every settling path.
///           * Listings are off-chain (gas-free) — only buy/cancel
///             touch chain state.
///           * Listing nonces tracked per-seller to bound replay.
///
/// @dev    This is a deliberately minimal contract — list/buy/cancel
///         only, no offers/auctions. Those belong in V2 once V1 is
///         audited live.
interface IERC721 {
    function ownerOf(uint256 tokenId) external view returns (address);
    function transferFrom(address from, address to, uint256 tokenId) external;
    function isApprovedForAll(address owner, address operator) external view returns (bool);
    function getApproved(uint256 tokenId) external view returns (address);
}

interface IERC2981 {
    function royaltyInfo(uint256 tokenId, uint256 salePrice)
        external view returns (address receiver, uint256 royaltyAmount);
}

contract ZbxNftMarketplace is ReentrancyGuard {

    /// Fee in basis points (e.g. 250 = 2.5%). Immutable so a
    /// compromised admin cannot raise it after the fact.
    uint16  public immutable feeBps;
    /// Hard ceiling enforced at construction.
    uint16  public constant MAX_FEE_BPS = 1000; // 10%

    address public immutable feeTreasury;

    /// Per-seller cancellation nonce. Bumping invalidates every
    /// outstanding signature with `nonce <= cancelledBefore[seller]`.
    mapping(address => uint256) public cancelledBefore;
    /// Per-(seller, nonce) one-shot consumption flag.
    mapping(address => mapping(uint256 => bool)) public consumed;

    // EIP-712-lite domain. Full EIP-712 is overkill for the v1 surface;
    // we use the same `\x19Ethereum Signed Message:\n32` convention as
    // ZbxBridge so wallet UX is identical.
    bytes32 public constant LISTING_TYPEHASH =
        keccak256("Listing(address seller,address nft,uint256 tokenId,address payToken,uint256 price,uint256 nonce,uint256 expiry)");

    event Sold(
        address indexed seller,
        address indexed buyer,
        address indexed nft,
        uint256 tokenId,
        address payToken,
        uint256 price,
        uint256 fee,
        uint256 royalty
    );
    event ListingCancelled(address indexed seller, uint256 nonce);
    event AllListingsCancelled(address indexed seller, uint256 throughNonce);

    error FeeTooHigh();
    error ZeroTreasury();
    error ListingExpired();
    error ListingCancelledErr();
    error ListingConsumed();
    error BadSignature();
    error NotOwner();
    error NotApproved();
    error PayTransferFailed();
    error PriceZero();

    constructor(uint16 feeBps_, address feeTreasury_) {
        if (feeBps_ > MAX_FEE_BPS)        revert FeeTooHigh();
        if (feeTreasury_ == address(0))   revert ZeroTreasury();
        feeBps      = feeBps_;
        feeTreasury = feeTreasury_;
    }

    /// @notice Buy an NFT against a seller's off-chain signature.
    /// @dev    Fee + royalty are computed and DEDUCTED FROM `price`
    ///         (seller receives `price - fee - royalty`). This is the
    ///         "fee-on-settle" semantic the audit spec requires —
    ///         no governance path can waive the fee, no path can
    ///         skip the royalty.
    function buy(
        address seller,
        address nft,
        uint256 tokenId,
        address payToken,
        uint256 price,
        uint256 nonce,
        uint256 expiry,
        bytes calldata sig
    ) external nonReentrant {
        if (price == 0)                       revert PriceZero();
        if (block.timestamp > expiry)         revert ListingExpired();
        if (nonce <= cancelledBefore[seller]) revert ListingCancelledErr();
        if (consumed[seller][nonce])          revert ListingConsumed();

        // Verify the seller actually signed THIS listing.
        bytes32 digest = keccak256(abi.encode(
            LISTING_TYPEHASH, seller, nft, tokenId, payToken, price, nonce, expiry,
            address(this), block.chainid
        ));
        bytes32 ethDigest = keccak256(abi.encodePacked(
            bytes1(0x19), "Ethereum Signed Message:\n32", digest
        ));
        if (_recover(ethDigest, sig) != seller) revert BadSignature();

        // Verify the seller still owns + has approved the NFT.
        if (IERC721(nft).ownerOf(tokenId) != seller) revert NotOwner();
        if (
            IERC721(nft).getApproved(tokenId)        != address(this) &&
            !IERC721(nft).isApprovedForAll(seller, address(this))
        ) revert NotApproved();

        consumed[seller][nonce] = true;

        // Fee-on-settle.
        uint256 fee = (price * feeBps) / 10_000;

        // EIP-2981 royalty (best-effort — staticcall + try/catch
        // semantics so non-2981 NFTs still trade with zero royalty).
        uint256 royalty;
        address royaltyReceiver;
        (bool ok, bytes memory ret) = nft.staticcall(
            abi.encodeWithSelector(IERC2981.royaltyInfo.selector, tokenId, price)
        );
        if (ok && ret.length >= 64) {
            (royaltyReceiver, royalty) = abi.decode(ret, (address, uint256));
            // Cap royalty at 10% to prevent griefing listings via
            // hostile NFT contracts — defense-in-depth.
            uint256 maxRoyalty = (price * 1000) / 10_000;
            if (royalty > maxRoyalty) royalty = maxRoyalty;
            if (royaltyReceiver == address(0)) royalty = 0;
        }

        uint256 sellerNet = price - fee - royalty;

        // Pull buyer's payToken in three legs.
        if (fee > 0) {
            if (!_pullPay(payToken, msg.sender, feeTreasury, fee))
                revert PayTransferFailed();
        }
        if (royalty > 0) {
            if (!_pullPay(payToken, msg.sender, royaltyReceiver, royalty))
                revert PayTransferFailed();
        }
        if (sellerNet > 0) {
            if (!_pullPay(payToken, msg.sender, seller, sellerNet))
                revert PayTransferFailed();
        }

        // Move the NFT.
        IERC721(nft).transferFrom(seller, msg.sender, tokenId);

        emit Sold(seller, msg.sender, nft, tokenId, payToken, price, fee, royalty);
    }

    /// @notice Cancel a single listing by its nonce.
    function cancelListing(uint256 nonce) external {
        consumed[msg.sender][nonce] = true;
        emit ListingCancelled(msg.sender, nonce);
    }

    /// @notice Cancel ALL listings with `nonce <= throughNonce`.
    ///         Emergency stop for a leaked seller key.
    function cancelAllUpTo(uint256 throughNonce) external {
        require(throughNonce >= cancelledBefore[msg.sender], "Marketplace: monotonic");
        cancelledBefore[msg.sender] = throughNonce;
        emit AllListingsCancelled(msg.sender, throughNonce);
    }

    // ─── Internals ───────────────────────────────────────────────────────

    function _pullPay(address payToken, address from, address to, uint256 amount)
        internal returns (bool)
    {
        // Minimal ERC-20 transferFrom — non-bool-returning tokens
        // (USDT) are handled by the bool-or-empty-return idiom.
        (bool ok, bytes memory ret) = payToken.call(
            abi.encodeWithSignature("transferFrom(address,address,uint256)", from, to, amount)
        );
        if (!ok) return false;
        if (ret.length == 0) return true;
        return abi.decode(ret, (bool));
    }

    function _recover(bytes32 digest, bytes calldata sig) internal pure returns (address) {
        if (sig.length != 65) return address(0);
        bytes32 r;
        bytes32 s;
        uint8   v;
        assembly {
            r := calldataload(sig.offset)
            s := calldataload(add(sig.offset, 32))
            v := byte(0, calldataload(add(sig.offset, 64)))
        }
        // EIP-2 low-s requirement.
        if (uint256(s) > 0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0) {
            return address(0);
        }
        if (v != 27 && v != 28) return address(0);
        return ecrecover(digest, v, r, s);
    }
}
