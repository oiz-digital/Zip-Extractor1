// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { ZRC20Base }      from "../ZRC20Base.sol";
import { IZRC20Mintable } from "../interfaces/IZRC20Mintable.sol";
import { IZRC20Burnable } from "../interfaces/IZRC20Burnable.sol";

/// @title ZbxBTC — Wrapped Bitcoin on Zebvix Chain
/// @notice Bridged representation of Bitcoin (via WBTC or native BTC bridge) on Zebvix Chain.
///         1 ZbxBTC is pegged 1:1 to 1 BTC.
///
/// @dev Bitcoin has 8 decimal places. ZBX Chain normalises to 18 decimals:
///      1 BTC = 1e18 ZbxBTC  (the bridge scales up by 1e10)
///
///      Custodian model: BTC is held by a multi-sig custodian (or BTC bridge protocol).
///      The BridgeVault contract authorises mints when BTC deposits are confirmed.
///      Minimum 6 confirmations required before mint.
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:ticker     BTC
/// @custom:decimals   18 (normalised from BTC's 8)
/// @custom:source     Bitcoin Network

contract ZbxBTC is ZRC20Base, IZRC20Mintable, IZRC20Burnable {

    address public owner;
    address public bridgeVault;
    mapping(address => bool) private _minters;

    // Hard cap: 21 million BTC (Bitcoin's maximum supply), normalised to 18 dec.
    uint256 public override mintCap = 21_000_000 * 1e18;
    uint256 private _totalBurned;

    // ─── Bridge metadata ──────────────────────────────────────────────────

    /// @notice Minimum BTC network confirmations before mint is authorised.
    uint8  public constant MIN_CONFIRMATIONS = 6;

    /// @notice Minimum mint amount to prevent dust (0.001 BTC).
    uint256 public constant MIN_MINT_AMOUNT  = 1e15;  // 0.001 * 1e18

    event OwnershipTransferred(address indexed prev, address indexed next);

    constructor(address bridgeVault_) ZRC20Base(
        "Wrapped Bitcoin",
        "BTC",
        18,
        "ipfs://QmBTCLogoXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX"
    ) {
        owner = msg.sender;
        bridgeVault = bridgeVault_;
        _minters[bridgeVault_] = true;
    }

    modifier onlyOwner()  { require(msg.sender == owner,  "ZbxBTC: not owner");  _; }
    modifier onlyMinter() { require(_minters[msg.sender], "ZbxBTC: not minter"); _; }

    function mint(address to, uint256 value) external override onlyMinter returns (bool) {
        require(value >= MIN_MINT_AMOUNT,         "ZbxBTC: below minimum");
        require(totalSupply() + value <= mintCap, "ZbxBTC: cap exceeded");
        _mint(to, value); emit Mint(to, value); return true;
    }

    function isMinter(address a) external view override returns (bool) { return _minters[a]; }
    function addMinter(address a) external override onlyOwner { _minters[a] = true; emit MinterAdded(a); }
    function removeMinter(address a) external override onlyOwner { _minters[a] = false; emit MinterRemoved(a); }

    function burn(uint256 value) external override returns (bool) {
        require(value >= MIN_MINT_AMOUNT, "ZbxBTC: below minimum withdrawal");
        _burn(msg.sender, value); unchecked { _totalBurned += value; }
        emit Burn(msg.sender, value); return true;
    }
    function burnFrom(address from, uint256 value) external override returns (bool) {
        require(value >= MIN_MINT_AMOUNT, "ZbxBTC: below minimum");
        _spendAllowance(from, msg.sender, value);
        _burn(from, value); unchecked { _totalBurned += value; }
        emit Burn(from, value); return true;
    }
    function totalBurned() external view override returns (uint256) { return _totalBurned; }

    /// @notice Convert satoshis (BTC 8-decimal) to ZbxBTC (18-decimal).
    function satoshisToZbxBTC(uint256 satoshis) external pure returns (uint256) {
        return satoshis * 1e10;
    }

    /// @notice Convert ZbxBTC (18-decimal) to satoshis (8-decimal).
    function zbxBTCToSatoshis(uint256 amount) external pure returns (uint256) {
        return amount / 1e10;
    }

    function transferOwnership(address to) external onlyOwner {
        emit OwnershipTransferred(owner, to); owner = to;
    }
    function _owner() internal view override returns (address) { return owner; }
}