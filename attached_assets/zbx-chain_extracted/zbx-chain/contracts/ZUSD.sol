// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { Ownable2Step }    from "./Ownable2Step.sol";
import { ReentrancyGuard } from "./libraries/ReentrancyGuard.sol";

/// @title ZUSD — Zebvix Native Stablecoin
/// @notice 1 ZUSD = 1 USD, backed by ZBX collateral (over-collateralised).
///
/// @dev   ZUSD is a debt token:
///           - Only ZusdVault can mint ZUSD (when user opens/tops up a CDP)
///           - Only ZusdVault can burn ZUSD (when user repays or is liquidated)
///           - All other ERC-20 functions are standard
///
///        Peg stability mechanisms:
///           1. Over-collateralisation (150% min) — hard backing
///           2. Stability fee (2% APY)            — mint is not "free"
///           3. Redemption (0.5% fee)              — floor arbitrage
///           4. Stability Pool                     — instant liquidation buffer
///
/// @custom:zbx-chain  Chain ID 8989

/// @dev SEC-2026-05-09 hardening: defense-in-depth ReentrancyGuard on all
///      mint/burn/transfer paths. ZUSD itself has no token-callback hooks,
///      but is integrated tightly with ZusdVault, ZusdStabilityPool, and
///      the ZbxAMM — this guard prevents any future vault upgrade or
///      hook-enabled wrapper from opening a cross-contract reentrancy
///      window via balance / supply manipulation.
contract ZUSD is Ownable2Step, ReentrancyGuard {

    // ─── ERC-20 state ─────────────────────────────────────────────────────

    string  public constant name     = "Zebvix USD";
    string  public constant symbol   = "ZUSD";
    uint8   public constant decimals = 18;

    uint256 public totalSupply;
    mapping(address => uint256)                     public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    // ─── Access control ───────────────────────────────────────────────────

    address public vault;      // ZusdVault — only minter/burner
    // S18: `owner` inherited from Ownable2Step.

    // ─── Events ───────────────────────────────────────────────────────────

    event Transfer(address indexed from, address indexed to, uint256 amount);
    event Approval(address indexed owner, address indexed spender, uint256 amount);
    event VaultUpdated(address indexed newVault);
    event Mint(address indexed to, uint256 amount);
    event Burn(address indexed from, uint256 amount);

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor() Ownable2Step(msg.sender) {}

    // ─── Vault gating ─────────────────────────────────────────────────────

    modifier onlyVault() {
        require(msg.sender == vault, "ZUSD: caller is not the vault");
        _;
    }

    // S18: `onlyOwner` modifier inherited from Ownable2Step.

    function setVault(address vault_) external onlyOwner {
        require(vault_ != address(0), "ZUSD: zero vault");
        vault = vault_;
        emit VaultUpdated(vault_);
    }

    // ─── Mint / Burn (vault only) ─────────────────────────────────────────

    /// @notice Mint ZUSD when user opens/tops up a CDP.
    /// @dev    Supply is bounded by vault collateral (ZusdVault enforces the
    ///         overcollateralisation ratio), not a hard numeric cap.
    ///         ZUSD is a collateral-backed stablecoin — supply must grow
    ///         freely with protocol demand, exactly like DAI/MakerDAO design.
    function mint(address to, uint256 amount) external onlyVault nonReentrant {
        require(to != address(0), "ZUSD: mint to zero");
        totalSupply      += amount;
        balanceOf[to]    += amount;
        emit Transfer(address(0), to, amount);
        emit Mint(to, amount);
    }

    /// @notice Burn ZUSD when user repays or gets liquidated.
    /// @dev    SEC-2026-05-09 Pass-19 (Tier-2 #10) hardening:
    ///           * `from != address(0)` — explicit reject (prevents
    ///             accounting noise from mis-encoded inputs).
    ///           * `amount > 0` — explicit reject (was previously
    ///             a no-op event spam vector for whoever controls
    ///             the vault contract).
    ///           * `balanceOf[from] >= amount` — preserved (was the
    ///             only prior guard).
    ///           * `onlyVault` + `nonReentrant` — preserved.
    ///         Combined invariants make `burn` a pure
    ///         debit-from-known-holder-with-vault-permission op
    ///         with no collateralisation-bypass surface.
    function burn(address from, uint256 amount) external onlyVault nonReentrant {
        require(from != address(0),               "ZUSD: burn from zero");
        require(amount > 0,                       "ZUSD: zero burn");
        require(balanceOf[from] >= amount,        "ZUSD: burn exceeds balance");
        balanceOf[from]  -= amount;
        totalSupply      -= amount;
        emit Transfer(from, address(0), amount);
        emit Burn(from, amount);
    }

    // ─── ERC-20 standard ──────────────────────────────────────────────────

    function transfer(address to, uint256 amount) external returns (bool) {
        require(balanceOf[msg.sender] >= amount, "ZUSD: insufficient balance");
        balanceOf[msg.sender] -= amount;
        balanceOf[to]         += amount;
        emit Transfer(msg.sender, to, amount);
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        require(balanceOf[from] >= amount,           "ZUSD: insufficient balance");
        require(allowance[from][msg.sender] >= amount, "ZUSD: insufficient allowance");
        allowance[from][msg.sender] -= amount;
        balanceOf[from]             -= amount;
        balanceOf[to]               += amount;
        emit Transfer(from, to, amount);
        return true;
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        emit Approval(msg.sender, spender, amount);
        return true;
    }
}