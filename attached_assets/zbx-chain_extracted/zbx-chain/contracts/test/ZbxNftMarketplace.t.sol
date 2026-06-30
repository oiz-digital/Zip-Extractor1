// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxNftMarketplace.sol";

interface IERC721Test {
    function balanceOf(address) external view returns (uint256);
    function ownerOf(uint256) external view returns (address);
    function transferFrom(address, address, uint256) external;
    function approve(address, uint256) external;
    function setApprovalForAll(address, bool) external;
    function isApprovedForAll(address, address) external view returns (bool);
}

contract MockNFT is IERC721Test {
    mapping(uint256 => address) private _owners;
    mapping(address => uint256) private _balances;
    mapping(uint256 => address) private _approved;
    mapping(address => mapping(address => bool)) private _operators;

    function mint(address to, uint256 tokenId) external {
        _owners[tokenId] = to;
        _balances[to]++;
    }

    function balanceOf(address owner) external view override returns (uint256) { return _balances[owner]; }
    function ownerOf(uint256 id) external view override returns (address) { return _owners[id]; }

    function approve(address to, uint256 id) external override {
        require(_owners[id] == msg.sender || _operators[_owners[id]][msg.sender]);
        _approved[id] = to;
    }

    function setApprovalForAll(address op, bool approved) external override {
        _operators[msg.sender][op] = approved;
    }

    function isApprovedForAll(address owner, address op) external view override returns (bool) {
        return _operators[owner][op];
    }

    function transferFrom(address from, address to, uint256 id) external override {
        require(_owners[id] == from);
        require(msg.sender == from || msg.sender == _approved[id] || _operators[from][msg.sender]);
        delete _approved[id];
        _balances[from]--;
        _balances[to]++;
        _owners[id] = to;
    }
}

contract ZbxNftMarketplaceTest is Test {
    ZbxNftMarketplace marketplace;
    MockNFT           nft;

    address admin = address(this);
    address alice = address(0xA11CE);
    address bob   = address(0xB0B);

    uint256 constant TOKEN_ID = 1;
    uint256 constant PRICE    = 1 ether;

    function setUp() public {
        marketplace = new ZbxNftMarketplace(admin, 250); // 2.5% protocol fee
        nft = new MockNFT();
        nft.mint(alice, TOKEN_ID);
        vm.deal(bob, 100 ether);

        vm.prank(alice);
        nft.setApprovalForAll(address(marketplace), true);
    }

    // ── List ──────────────────────────────────────────────────────────────

    function test_list_nft() public {
        vm.prank(alice);
        marketplace.list(address(nft), TOKEN_ID, PRICE);
        (address seller, uint256 price,) = marketplace.listing(address(nft), TOKEN_ID);
        assertEq(seller, alice);
        assertEq(price, PRICE);
    }

    function test_list_zero_price_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        marketplace.list(address(nft), TOKEN_ID, 0);
    }

    function test_non_owner_cannot_list() public {
        vm.prank(bob);
        vm.expectRevert();
        marketplace.list(address(nft), TOKEN_ID, PRICE);
    }

    // ── Buy ───────────────────────────────────────────────────────────────

    function test_buy_transfers_nft() public {
        vm.prank(alice);
        marketplace.list(address(nft), TOKEN_ID, PRICE);
        vm.prank(bob);
        marketplace.buy{value: PRICE}(address(nft), TOKEN_ID);
        assertEq(nft.ownerOf(TOKEN_ID), bob);
    }

    function test_buy_pays_seller() public {
        vm.prank(alice);
        marketplace.list(address(nft), TOKEN_ID, PRICE);
        uint256 before = alice.balance;
        vm.prank(bob);
        marketplace.buy{value: PRICE}(address(nft), TOKEN_ID);
        assertGt(alice.balance, before);
    }

    function test_buy_wrong_price_reverts() public {
        vm.prank(alice);
        marketplace.list(address(nft), TOKEN_ID, PRICE);
        vm.prank(bob);
        vm.expectRevert();
        marketplace.buy{value: PRICE / 2}(address(nft), TOKEN_ID);
    }

    function test_buy_unlisted_reverts() public {
        vm.prank(bob);
        vm.expectRevert();
        marketplace.buy{value: PRICE}(address(nft), TOKEN_ID);
    }

    // ── Delist ────────────────────────────────────────────────────────────

    function test_delist_removes_listing() public {
        vm.prank(alice);
        marketplace.list(address(nft), TOKEN_ID, PRICE);
        vm.prank(alice);
        marketplace.delist(address(nft), TOKEN_ID);
        (address seller,,) = marketplace.listing(address(nft), TOKEN_ID);
        assertEq(seller, address(0));
    }

    function test_non_seller_cannot_delist() public {
        vm.prank(alice);
        marketplace.list(address(nft), TOKEN_ID, PRICE);
        vm.prank(bob);
        vm.expectRevert();
        marketplace.delist(address(nft), TOKEN_ID);
    }

    // ── Update price ──────────────────────────────────────────────────────

    function test_update_price() public {
        vm.prank(alice);
        marketplace.list(address(nft), TOKEN_ID, PRICE);
        vm.prank(alice);
        marketplace.updatePrice(address(nft), TOKEN_ID, 2 ether);
        (, uint256 newPrice,) = marketplace.listing(address(nft), TOKEN_ID);
        assertEq(newPrice, 2 ether);
    }

    // ── Protocol fee ──────────────────────────────────────────────────────

    function test_protocol_fee_collected() public {
        vm.prank(alice);
        marketplace.list(address(nft), TOKEN_ID, PRICE);
        vm.prank(bob);
        marketplace.buy{value: PRICE}(address(nft), TOKEN_ID);
        assertGt(marketplace.accruedFees(), 0);
    }

    function test_owner_can_withdraw_fees() public {
        vm.prank(alice);
        marketplace.list(address(nft), TOKEN_ID, PRICE);
        vm.prank(bob);
        marketplace.buy{value: PRICE}(address(nft), TOKEN_ID);
        uint256 before = admin.balance;
        marketplace.withdrawFees(payable(admin));
        assertGt(admin.balance, before);
    }
}
