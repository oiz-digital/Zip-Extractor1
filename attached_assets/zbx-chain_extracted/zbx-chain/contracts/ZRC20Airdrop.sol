// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { IZRC20 } from "./interfaces/IZRC20.sol";

/// @title ZRC20Airdrop — Efficient Merkle-proof airdrop for ZRC-20 tokens.
/// @notice Distribute tokens to thousands of addresses cheaply.
///         Only the Merkle root is stored on-chain.
///         Recipients claim by submitting a proof.
///
/// @dev Gas cost per claim: ~45 000 ZBX Chain gas.
///      Off-chain: compute Merkle tree from (address, amount) pairs.
///      Keccak256(abi.encodePacked(address, amount)) = leaf.

contract ZRC20Airdrop {

    // ─── Events ───────────────────────────────────────────────────────────

    event AirdropCreated(uint256 indexed id, address token, bytes32 root, uint256 totalAmount, uint64 expiry);
    event Claimed(uint256 indexed id, address indexed claimant, uint256 amount);
    event Expired(uint256 indexed id, uint256 returned);

    // ─── State ────────────────────────────────────────────────────────────

    struct Airdrop {
        IZRC20  token;
        bytes32 merkleRoot;
        uint256 totalAmount;
        uint256 claimedAmount;
        uint64  expiry;         // unix timestamp — unclaimed tokens returned after this
        bool    active;
    }

    address public owner;
    uint256 public airdropCount;

    mapping(uint256 => Airdrop)          public airdrops;
    mapping(uint256 => mapping(address => bool)) public hasClaimed;

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor() { owner = msg.sender; }

    // ─── Create Airdrop ───────────────────────────────────────────────────

    function createAirdrop(
        address token,
        bytes32 merkleRoot,
        uint256 totalAmount,
        uint64  expiry
    ) external returns (uint256 id) {
        require(msg.sender == owner,       "Airdrop: not owner");
        require(expiry > block.timestamp,  "Airdrop: expiry in past");
        require(totalAmount > 0,           "Airdrop: zero amount");

        id = airdropCount++;
        airdrops[id] = Airdrop({
            token:         IZRC20(token),
            merkleRoot:    merkleRoot,
            totalAmount:   totalAmount,
            claimedAmount: 0,
            expiry:        expiry,
            active:        true
        });

        IZRC20(token).transferFrom(msg.sender, address(this), totalAmount);
        emit AirdropCreated(id, token, merkleRoot, totalAmount, expiry);
    }

    // ─── Claim ────────────────────────────────────────────────────────────

    function claim(uint256 id, uint256 amount, bytes32[] calldata proof) external {
        Airdrop storage a = airdrops[id];
        require(a.active,                          "Airdrop: not active");
        require(block.timestamp < a.expiry,        "Airdrop: expired");
        require(!hasClaimed[id][msg.sender],       "Airdrop: already claimed");

        // Verify Merkle proof.
        bytes32 leaf = keccak256(abi.encodePacked(msg.sender, amount));
        require(_verify(a.merkleRoot, leaf, proof), "Airdrop: invalid proof");

        hasClaimed[id][msg.sender] = true;
        a.claimedAmount += amount;
        a.token.transfer(msg.sender, amount);

        emit Claimed(id, msg.sender, amount);
    }

    // ─── Expire ───────────────────────────────────────────────────────────

    function expireAirdrop(uint256 id) external {
        Airdrop storage a = airdrops[id];
        require(msg.sender == owner,         "Airdrop: not owner");
        require(a.active,                    "Airdrop: not active");
        require(block.timestamp >= a.expiry, "Airdrop: not yet expired");

        a.active = false;
        uint256 remaining = a.totalAmount - a.claimedAmount;
        if (remaining > 0) {
            a.token.transfer(owner, remaining);
        }
        emit Expired(id, remaining);
    }

    // ─── Internal: Merkle verification ────────────────────────────────────

    function _verify(bytes32 root, bytes32 leaf, bytes32[] calldata proof) internal pure returns (bool) {
        bytes32 hash = leaf;
        for (uint256 i; i < proof.length; ++i) {
            bytes32 p = proof[i];
            hash = hash < p
                ? keccak256(abi.encodePacked(hash, p))
                : keccak256(abi.encodePacked(p, hash));
        }
        return hash == root;
    }
}