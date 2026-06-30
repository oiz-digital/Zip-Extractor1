// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxRaffle.sol";

contract MockVRF {
    mapping(bytes32 => bool) public revealable;
    mapping(bytes32 => uint256) public randomResults;

    function requestRandom(bytes32) external returns (bytes32 requestId) {
        requestId = keccak256(abi.encodePacked(block.timestamp, msg.sender));
        revealable[requestId] = true;
        randomResults[requestId] = uint256(keccak256(abi.encodePacked(requestId)));
        return requestId;
    }

    function fulfillRandom(bytes32 requestId, bytes32) external returns (uint256) {
        return randomResults[requestId];
    }

    function isRevealable(bytes32 requestId) external view returns (bool) {
        return revealable[requestId];
    }
}

contract ZbxRaffleTest is Test {
    ZbxRaffle raffle;
    MockVRF   vrf;

    address admin   = address(this);
    address creator = address(0xC4EA704);
    address alice   = address(0xA11CE);
    address bob     = address(0xB0B);
    address carol   = address(0xCA401);

    uint256 constant TICKET_PRICE = 0.1 ether;
    uint256 constant MAX_TICKETS  = 100;

    function setUp() public {
        vrf    = new MockVRF();
        raffle = new ZbxRaffle(admin, address(vrf));

        vm.deal(creator, 10 ether);
        vm.deal(alice,   10 ether);
        vm.deal(bob,     10 ether);
        vm.deal(carol,   10 ether);
    }

    function _createRaffle() internal returns (uint256 id) {
        vm.prank(creator);
        id = raffle.createRaffle{value: 0}(
            address(0),    // native ZBX tickets
            TICKET_PRICE,
            MAX_TICKETS,
            block.timestamp + 7 days
        );
    }

    // ── Create ────────────────────────────────────────────────────────────

    function test_create_raffle() public {
        uint256 id = _createRaffle();
        assertGt(id, 0);
    }

    function test_create_zero_ticket_price_reverts() public {
        vm.prank(creator);
        vm.expectRevert();
        raffle.createRaffle{value: 0}(address(0), 0, MAX_TICKETS, block.timestamp + 7 days);
    }

    function test_create_deadline_in_past_reverts() public {
        vm.prank(creator);
        vm.expectRevert();
        raffle.createRaffle{value: 0}(address(0), TICKET_PRICE, MAX_TICKETS, block.timestamp - 1);
    }

    // ── Buy tickets ───────────────────────────────────────────────────────

    function test_buy_ticket() public {
        uint256 id = _createRaffle();
        vm.prank(alice);
        raffle.buyTickets{value: TICKET_PRICE}(id, 1);
        assertEq(raffle.ticketCount(id), 1);
    }

    function test_buy_multiple_tickets() public {
        uint256 id = _createRaffle();
        vm.prank(alice);
        raffle.buyTickets{value: TICKET_PRICE * 5}(id, 5);
        assertEq(raffle.ticketCount(id), 5);
    }

    function test_buy_wrong_amount_reverts() public {
        uint256 id = _createRaffle();
        vm.prank(alice);
        vm.expectRevert();
        raffle.buyTickets{value: TICKET_PRICE / 2}(id, 1);
    }

    function test_buy_after_deadline_reverts() public {
        uint256 id = _createRaffle();
        vm.warp(block.timestamp + 8 days);
        vm.prank(alice);
        vm.expectRevert();
        raffle.buyTickets{value: TICKET_PRICE}(id, 1);
    }

    function test_max_tickets_enforced() public {
        uint256 id = _createRaffle();
        vm.prank(alice);
        raffle.buyTickets{value: TICKET_PRICE * MAX_TICKETS}(id, MAX_TICKETS);
        vm.prank(bob);
        vm.expectRevert();
        raffle.buyTickets{value: TICKET_PRICE}(id, 1);
    }

    // ── Draw ──────────────────────────────────────────────────────────────

    function test_initiate_draw_after_deadline() public {
        uint256 id = _createRaffle();
        // Buy minimum tickets
        vm.prank(alice);
        raffle.buyTickets{value: TICKET_PRICE * 3}(id, 3);
        vm.prank(bob);
        raffle.buyTickets{value: TICKET_PRICE * 3}(id, 3);
        vm.prank(carol);
        raffle.buyTickets{value: TICKET_PRICE * 3}(id, 3);

        vm.warp(block.timestamp + 8 days);
        vm.prank(creator);
        raffle.initiateDraw(id, keccak256("seed"));
        // VRF request initiated, no revert
    }

    function test_draw_before_deadline_reverts() public {
        uint256 id = _createRaffle();
        vm.prank(alice);
        raffle.buyTickets{value: TICKET_PRICE * 3}(id, 3);
        vm.prank(creator);
        vm.expectRevert();
        raffle.initiateDraw(id, keccak256("seed"));
    }

    function test_draw_without_enough_participants_reverts() public {
        uint256 id = _createRaffle();
        vm.prank(alice);
        raffle.buyTickets{value: TICKET_PRICE}(id, 1); // only 1 ticket
        vm.warp(block.timestamp + 8 days);
        vm.prank(creator);
        vm.expectRevert();
        raffle.initiateDraw(id, keccak256("seed"));
    }

    // ── Cancel ────────────────────────────────────────────────────────────

    function test_cancel_refunds_tickets() public {
        uint256 id = _createRaffle();
        vm.prank(alice);
        raffle.buyTickets{value: TICKET_PRICE * 2}(id, 2);

        uint256 before = alice.balance;
        vm.prank(creator);
        raffle.cancelRaffle(id);

        vm.prank(alice);
        raffle.refund(id);
        assertEq(alice.balance, before + TICKET_PRICE * 2);
    }

    function test_non_creator_cannot_cancel() public {
        uint256 id = _createRaffle();
        vm.prank(alice);
        vm.expectRevert();
        raffle.cancelRaffle(id);
    }

    // ── Ticket owner ──────────────────────────────────────────────────────

    function test_ticket_owner_tracked() public {
        uint256 id = _createRaffle();
        vm.prank(alice);
        raffle.buyTickets{value: TICKET_PRICE}(id, 1);
        assertEq(raffle.ticketOwner(id, 0), alice);
    }
}
