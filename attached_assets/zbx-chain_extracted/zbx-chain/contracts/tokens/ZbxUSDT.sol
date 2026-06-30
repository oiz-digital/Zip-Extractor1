// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { ZRC20Base }       from "../ZRC20Base.sol";
import { IZRC20Mintable }  from "../interfaces/IZRC20Mintable.sol";
import { IZRC20Burnable }  from "../interfaces/IZRC20Burnable.sol";

/// @title ZbxUSDT — Tether USD on Zebvix Chain
/// @notice Bridged representation of USDT (Tether) on Zebvix Chain.
///         Minted by BridgeVault when USDT is locked on Ethereum/BSC/Tron.
///         Burned when the holder bridges back to any supported chain.
///
/// @dev Supply on ZBX Chain:
///         ZbxUSDT.totalSupply() ≤ Σ USDT locked across all bridge vaults
///
///      Decimals: 18 (unlike mainnet USDT's 6) — ZBX Chain normalises all
///      stablecoins to 18 decimals for AMM compatibility.
///      The bridge layer handles the 10^12 scaling factor.
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:ticker     USDT
/// @custom:decimals   18
/// @custom:peg        1 ZbxUSDT = 1 USD (soft peg via bridge)

contract ZbxUSDT is ZRC20Base, IZRC20Mintable, IZRC20Burnable {

    // ─── Roles ────────────────────────────────────────────────────────────

    address public owner;
    address public bridgeVault;          // only minter/burner in normal operation
    mapping(address => bool) private _minters;

    uint256 public override mintCap = 10_000_000_000 * 1e18; // 10 billion USDT cap
    uint256 private _totalBurned;

    // ─── Events ───────────────────────────────────────────────────────────

    event BridgeVaultUpdated(address indexed prev, address indexed next);
    event OwnershipTransferred(address indexed prev, address indexed next);

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor(address bridgeVault_) ZRC20Base(
        "Tether USD",
        "USDT",
        18,
        "ipfs://QmUSDTLogoXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX"
    ) {
        require(bridgeVault_ != address(0), "ZbxUSDT: zero vault");
        owner       = msg.sender;
        bridgeVault = bridgeVault_;
        _minters[bridgeVault_] = true;
    }

    modifier onlyOwner()  { require(msg.sender == owner,        "ZbxUSDT: not owner");  _; }
    modifier onlyMinter() { require(_minters[msg.sender],       "ZbxUSDT: not minter"); _; }

    // ─── IZRC20Mintable ───────────────────────────────────────────────────

    function mint(address to, uint256 value) external override onlyMinter returns (bool) {
        require(totalSupply() + value <= mintCap, "ZbxUSDT: cap exceeded");
        _mint(to, value);
        emit Mint(to, value);
        return true;
    }

    function isMinter(address a) external view override returns (bool) { return _minters[a]; }
    function addMinter(address a) external override onlyOwner {
        _minters[a] = true; emit MinterAdded(a);
    }
    function removeMinter(address a) external override onlyOwner {
        _minters[a] = false; emit MinterRemoved(a);
    }

    // ─── IZRC20Burnable ───────────────────────────────────────────────────

    function burn(uint256 value) external override returns (bool) {
        _burn(msg.sender, value);
        unchecked { _totalBurned += value; }
        emit Burn(msg.sender, value);
        return true;
    }

    function burnFrom(address from, uint256 value) external override returns (bool) {
        _spendAllowance(from, msg.sender, value);
        _burn(from, value);
        unchecked { _totalBurned += value; }
        emit Burn(from, value);
        return true;
    }

    function totalBurned() external view override returns (uint256) { return _totalBurned; }

    // ─── Admin ────────────────────────────────────────────────────────────

    function setBridgeVault(address vault) external onlyOwner {
        require(vault != address(0), "ZbxUSDT: zero vault");
        _minters[bridgeVault] = false;
        emit MinterRemoved(bridgeVault);
        bridgeVault = vault;
        _minters[vault] = true;
        emit BridgeVaultUpdated(bridgeVault, vault);
        emit MinterAdded(vault);
    }

    function transferOwnership(address to) external onlyOwner {
        emit OwnershipTransferred(owner, to); owner = to;
    }

    function _owner() internal view override returns (address) { return owner; }
}