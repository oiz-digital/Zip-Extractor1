// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { IZRC20 } from "./interfaces/IZRC20.sol";

/// @title ZRC20Vesting — Token vesting with cliff and linear release.
/// @notice Used for team, investor, and ecosystem allocations on Zebvix Chain.
///         Tokens unlock linearly after a cliff period.
///
/// @dev Vesting schedule:
///         0 ─────── cliff ──────── start+duration
///         |  0 tokens  |  linear release  |  100% released

contract ZRC20Vesting {

    // ─── Events ───────────────────────────────────────────────────────────

    event GrantCreated(address indexed beneficiary, uint256 amount, uint64 start, uint64 cliff, uint64 duration);
    event TokensReleased(address indexed beneficiary, uint256 amount);
    event GrantRevoked(address indexed beneficiary, uint256 returned);

    // ─── Vesting Grant ────────────────────────────────────────────────────

    struct Grant {
        uint256 amount;       // total tokens granted
        uint256 released;     // tokens already released
        uint64  start;        // unix timestamp of vesting start
        uint64  cliff;        // cliff duration in seconds
        uint64  duration;     // total vesting duration in seconds
        bool    revocable;    // can owner revoke?
        bool    revoked;
    }

    // ─── State ────────────────────────────────────────────────────────────

    IZRC20  public immutable token;
    address public           owner;

    mapping(address => Grant) public grants;

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor(address token_) {
        token = IZRC20(token_);
        owner = msg.sender;
    }

    // ─── Grant ────────────────────────────────────────────────────────────

    function createGrant(
        address beneficiary,
        uint256 amount,
        uint64  start,
        uint64  cliff,
        uint64  duration,
        bool    revocable
    ) external {
        require(msg.sender == owner,          "Vesting: not owner");
        require(beneficiary != address(0),    "Vesting: zero address");
        require(amount > 0,                   "Vesting: zero amount");
        require(duration > 0,                 "Vesting: zero duration");
        require(cliff <= duration,            "Vesting: cliff > duration");
        require(grants[beneficiary].amount == 0, "Vesting: grant exists");

        grants[beneficiary] = Grant({
            amount:    amount,
            released:  0,
            start:     start == 0 ? uint64(block.timestamp) : start,
            cliff:     cliff,
            duration:  duration,
            revocable: revocable,
            revoked:   false
        });

        require(token.transferFrom(msg.sender, address(this), amount), "Vesting: transferFrom failed");
        emit GrantCreated(beneficiary, amount, start, cliff, duration);
    }

    // ─── Release ──────────────────────────────────────────────────────────

    function release() external {
        Grant storage g = grants[msg.sender];
        require(g.amount > 0,  "Vesting: no grant");
        require(!g.revoked,    "Vesting: revoked");

        uint256 releasable = _releasable(g);
        require(releasable > 0, "Vesting: nothing to release");

        g.released += releasable;
        require(token.transfer(msg.sender, releasable), "Vesting: transfer failed");
        emit TokensReleased(msg.sender, releasable);
    }

    /// @notice How many tokens can be released right now.
    function releasable(address beneficiary) external view returns (uint256) {
        return _releasable(grants[beneficiary]);
    }

    /// @notice Total tokens vested (released + releasable) at current time.
    function vested(address beneficiary) external view returns (uint256) {
        return _vested(grants[beneficiary], uint64(block.timestamp));
    }

    // ─── Revoke ───────────────────────────────────────────────────────────

    function revoke(address beneficiary) external {
        require(msg.sender == owner, "Vesting: not owner");
        Grant storage g = grants[beneficiary];
        require(g.revocable,  "Vesting: not revocable");
        require(!g.revoked,   "Vesting: already revoked");

        uint256 vestedNow = _vested(g, uint64(block.timestamp));
        uint256 unvested  = g.amount - vestedNow;

        g.revoked = true;
        if (unvested > 0) {
            require(token.transfer(owner, unvested), "Vesting: revoke transfer failed");
            emit GrantRevoked(beneficiary, unvested);
        }
    }

    // ─── Internals ────────────────────────────────────────────────────────

    function _releasable(Grant storage g) private view returns (uint256) {
        return _vested(g, uint64(block.timestamp)) - g.released;
    }

    function _vested(Grant storage g, uint64 ts) private view returns (uint256) {
        if (g.amount == 0 || ts < g.start + g.cliff) return 0;
        if (ts >= g.start + g.duration) return g.amount;
        return g.amount * (ts - g.start) / g.duration;
    }
}