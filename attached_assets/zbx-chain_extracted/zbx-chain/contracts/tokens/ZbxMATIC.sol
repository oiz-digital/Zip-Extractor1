// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { ZRC20Base }      from "../ZRC20Base.sol";
import { IZRC20Mintable } from "../interfaces/IZRC20Mintable.sol";
import { IZRC20Burnable } from "../interfaces/IZRC20Burnable.sol";

/// @title ZbxMATIC — Polygon (POL/MATIC) on Zebvix Chain
/// @notice Bridged representation of POL (formerly MATIC) from Polygon network.
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:ticker     MATIC
/// @custom:decimals   18
/// @custom:source     Polygon PoS (Chain ID 137)

contract ZbxMATIC is ZRC20Base, IZRC20Mintable, IZRC20Burnable {

    address public owner;
    address public bridgeVault;
    mapping(address => bool) private _minters;

    uint256 public override mintCap = 10_000_000_000 * 1e18; // 10B POL total supply
    uint256 private _totalBurned;

    event OwnershipTransferred(address indexed prev, address indexed next);

    constructor(address bridgeVault_) ZRC20Base(
        "Polygon",
        "MATIC",
        18,
        "ipfs://QmMATICLogoXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX"
    ) {
        owner = msg.sender;
        bridgeVault = bridgeVault_;
        _minters[bridgeVault_] = true;
    }

    modifier onlyOwner()  { require(msg.sender == owner,  "ZbxMATIC: not owner");  _; }
    modifier onlyMinter() { require(_minters[msg.sender], "ZbxMATIC: not minter"); _; }

    function mint(address to, uint256 value) external override onlyMinter returns (bool) {
        require(totalSupply() + value <= mintCap, "ZbxMATIC: cap exceeded");
        _mint(to, value); emit Mint(to, value); return true;
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

    function transferOwnership(address to) external onlyOwner {
        emit OwnershipTransferred(owner, to); owner = to;
    }
    function _owner() internal view override returns (address) { return owner; }
}