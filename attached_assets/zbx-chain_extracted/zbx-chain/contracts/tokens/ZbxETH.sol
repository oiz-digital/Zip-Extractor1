// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { ZRC20Base }      from "../ZRC20Base.sol";
import { IZRC20Mintable } from "../interfaces/IZRC20Mintable.sol";
import { IZRC20Burnable } from "../interfaces/IZRC20Burnable.sol";

/// @title ZbxETH — Wrapped Ether on Zebvix Chain
/// @notice Bridged representation of ETH on Zebvix Chain.
///         Minted when ETH is locked in the Ethereum BridgeVault.
///         Burned when user bridges ZbxETH back to Ethereum.
///
/// @dev 1 ZbxETH == 1 ETH (maintained by bridge).
///      Decimals: 18 (same as native ETH).
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:ticker     ETH
/// @custom:decimals   18
/// @custom:source     Ethereum Mainnet

contract ZbxETH is ZRC20Base, IZRC20Mintable, IZRC20Burnable {

    address public owner;
    address public bridgeVault;
    mapping(address => bool) private _minters;
    uint256 public override mintCap = 21_000_000 * 1e18; // hard cap: more ETH than exists
    uint256 private _totalBurned;

    event OwnershipTransferred(address indexed prev, address indexed next);
    event OwnershipTransferStarted(address indexed prev, address indexed next);

    address public pendingOwner;

    constructor(address bridgeVault_) ZRC20Base(
        "Ether",
        "ETH",
        18,
        "ipfs://QmETHLogoXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX"
    ) {
        owner = msg.sender;
        bridgeVault = bridgeVault_;
        _minters[bridgeVault_] = true;
    }

    modifier onlyOwner()  { require(msg.sender == owner,  "ZbxETH: not owner");  _; }
    modifier onlyMinter() { require(_minters[msg.sender], "ZbxETH: not minter"); _; }

    function mint(address to, uint256 value) external override onlyMinter returns (bool) {
        require(totalSupply() + value <= mintCap, "ZbxETH: cap exceeded");
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
        require(to != address(0), "ZbxETH: zero address");
        pendingOwner = to;
        emit OwnershipTransferStarted(owner, to);
    }
    function acceptOwnership() external {
        require(msg.sender == pendingOwner, "ZbxETH: not pending owner");
        emit OwnershipTransferred(owner, pendingOwner);
        owner        = pendingOwner;
        pendingOwner = address(0);
    }
    function _owner() internal view override returns (address) { return owner; }
}