// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { ZRC20Base }      from "../ZRC20Base.sol";
import { IZRC20Mintable } from "../interfaces/IZRC20Mintable.sol";
import { IZRC20Burnable } from "../interfaces/IZRC20Burnable.sol";

/// @title ZbxUSDC — USD Coin on Zebvix Chain
/// @notice Bridged representation of USDC (Circle) on Zebvix Chain.
///         Minted by BridgeVault when USDC is locked on Ethereum, Arbitrum, or Polygon.
///         Circle's cross-chain transfer protocol (CCTP) is supported via the bridge adapter.
///
/// @dev Decimals: 18 (normalised from USDC's 6).
///      Blacklist: Circle maintains the right to freeze addresses on mainnet USDC.
///      ZbxUSDC inherits that blacklist enforcement via `_beforeTransfer`.
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:ticker     USDC
/// @custom:decimals   18
/// @custom:issuer     Circle Internet Financial (bridged representation)

contract ZbxUSDC is ZRC20Base, IZRC20Mintable, IZRC20Burnable {

    address public owner;
    address public bridgeVault;
    mapping(address => bool) private _minters;
    mapping(address => bool) public  blacklisted;   // mirrors Circle blacklist

    uint256 public override mintCap = 10_000_000_000 * 1e18;
    uint256 private _totalBurned;

    event Blacklisted(address indexed account);
    event UnBlacklisted(address indexed account);
    event OwnershipTransferred(address indexed prev, address indexed next);

    constructor(address bridgeVault_) ZRC20Base(
        "USD Coin",
        "USDC",
        18,
        "ipfs://QmUSDCLogoXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX"
    ) {
        owner = msg.sender;
        bridgeVault = bridgeVault_;
        _minters[bridgeVault_] = true;
    }

    modifier onlyOwner()  { require(msg.sender == owner,  "ZbxUSDC: not owner");  _; }
    modifier onlyMinter() { require(_minters[msg.sender], "ZbxUSDC: not minter"); _; }

    function mint(address to, uint256 value) external override onlyMinter returns (bool) {
        require(!blacklisted[to],                 "ZbxUSDC: to blacklisted");
        require(totalSupply() + value <= mintCap, "ZbxUSDC: cap exceeded");
        _mint(to, value);
        emit Mint(to, value);
        return true;
    }

    function isMinter(address a) external view override returns (bool) { return _minters[a]; }
    function addMinter(address a) external override onlyOwner { _minters[a] = true; emit MinterAdded(a); }
    function removeMinter(address a) external override onlyOwner { _minters[a] = false; emit MinterRemoved(a); }

    function burn(uint256 value) external override returns (bool) {
        _burn(msg.sender, value); unchecked { _totalBurned += value; }
        emit Burn(msg.sender, value); return true;
    }
    function burnFrom(address from, uint256 value) external override returns (bool) {
        _spendAllowance(from, msg.sender, value);
        _burn(from, value); unchecked { _totalBurned += value; }
        emit Burn(from, value); return true;
    }
    function totalBurned() external view override returns (uint256) { return _totalBurned; }

    // ─── Blacklist (Circle compliance) ───────────────────────────────────

    function blacklist(address account) external onlyOwner {
        blacklisted[account] = true; emit Blacklisted(account);
    }
    function unBlacklist(address account) external onlyOwner {
        blacklisted[account] = false; emit UnBlacklisted(account);
    }

    function _beforeTransfer(address from, address to, uint256) internal override {
        require(!blacklisted[from], "ZbxUSDC: from blacklisted");
        require(!blacklisted[to],   "ZbxUSDC: to blacklisted");
    }

    function transferOwnership(address to) external onlyOwner {
        emit OwnershipTransferred(owner, to); owner = to;
    }
    function _owner() internal view override returns (address) { return owner; }
}