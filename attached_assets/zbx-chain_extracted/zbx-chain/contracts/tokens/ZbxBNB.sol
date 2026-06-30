// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { ZRC20Base }      from "../ZRC20Base.sol";
import { IZRC20Mintable } from "../interfaces/IZRC20Mintable.sol";
import { IZRC20Burnable } from "../interfaces/IZRC20Burnable.sol";

/// @title ZbxBNB — BNB on Zebvix Chain
/// @notice Bridged representation of BNB (BNB Chain) on Zebvix Chain.
///         Minted when BNB is locked in the BNB Chain BridgeVault.
///         Burned when user bridges ZbxBNB back to BNB Chain.
///
/// @dev 1 ZbxBNB = 1 BNB. Decimals: 18 (same as BNB).
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:ticker     BNB
/// @custom:decimals   18
/// @custom:source     BNB Chain (Chain ID 56)

contract ZbxBNB is ZRC20Base, IZRC20Mintable, IZRC20Burnable {

    address public owner;
    address public bridgeVault;
    mapping(address => bool) private _minters;

    uint256 public override mintCap = 200_000_000 * 1e18; // BNB max supply cap
    uint256 private _totalBurned;

    event OwnershipTransferred(address indexed prev, address indexed next);

    constructor(address bridgeVault_) ZRC20Base(
        "BNB",
        "BNB",
        18,
        "ipfs://QmBNBLogoXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX"
    ) {
        owner = msg.sender;
        bridgeVault = bridgeVault_;
        _minters[bridgeVault_] = true;
    }

    modifier onlyOwner()  { require(msg.sender == owner,  "ZbxBNB: not owner");  _; }
    modifier onlyMinter() { require(_minters[msg.sender], "ZbxBNB: not minter"); _; }

    function mint(address to, uint256 value) external override onlyMinter returns (bool) {
        require(totalSupply() + value <= mintCap, "ZbxBNB: cap exceeded");
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