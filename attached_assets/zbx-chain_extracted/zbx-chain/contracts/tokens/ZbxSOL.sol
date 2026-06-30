// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { ZRC20Base }      from "../ZRC20Base.sol";
import { IZRC20Mintable } from "../interfaces/IZRC20Mintable.sol";
import { IZRC20Burnable } from "../interfaces/IZRC20Burnable.sol";

/// @title ZbxSOL — Solana (SOL) on Zebvix Chain
/// @notice Bridged representation of SOL on Zebvix Chain.
///         Minted when SOL is locked in the Solana-side bridge program.
///         Burned when user bridges back to Solana.
///
/// @dev Solana uses lamports (1 SOL = 1e9 lamports).
///      ZBX Chain normalises to 18 decimals (1 SOL = 1e18 ZbxSOL).
///      The bridge adapter scales by 1e9.
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:ticker     SOL
/// @custom:decimals   18
/// @custom:source     Solana Mainnet-Beta

contract ZbxSOL is ZRC20Base, IZRC20Mintable, IZRC20Burnable {

    address public owner;
    address public bridgeVault;
    mapping(address => bool) private _minters;

    uint256 public override mintCap = 600_000_000 * 1e18;
    uint256 private _totalBurned;

    uint256 public constant MIN_BRIDGE_AMOUNT = 1e16; // 0.01 SOL minimum

    event OwnershipTransferred(address indexed prev, address indexed next);

    constructor(address bridgeVault_) ZRC20Base(
        "Solana",
        "SOL",
        18,
        "ipfs://QmSOLLogoXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX"
    ) {
        owner = msg.sender;
        bridgeVault = bridgeVault_;
        _minters[bridgeVault_] = true;
    }

    modifier onlyOwner()  { require(msg.sender == owner,  "ZbxSOL: not owner");  _; }
    modifier onlyMinter() { require(_minters[msg.sender], "ZbxSOL: not minter"); _; }

    function mint(address to, uint256 value) external override onlyMinter returns (bool) {
        require(value >= MIN_BRIDGE_AMOUNT,       "ZbxSOL: below minimum");
        require(totalSupply() + value <= mintCap, "ZbxSOL: cap exceeded");
        _mint(to, value); emit Mint(to, value); return true;
    }

    function isMinter(address a) external view override returns (bool) { return _minters[a]; }
    function addMinter(address a) external override onlyOwner { _minters[a] = true; emit MinterAdded(a); }
    function removeMinter(address a) external override onlyOwner { _minters[a] = false; emit MinterRemoved(a); }

    function burn(uint256 value) external override returns (bool) {
        require(value >= MIN_BRIDGE_AMOUNT, "ZbxSOL: below minimum");
        _burn(msg.sender, value); unchecked { _totalBurned += value; }
        emit Burn(msg.sender, value); return true;
    }
    function burnFrom(address from, uint256 value) external override returns (bool) {
        require(value >= MIN_BRIDGE_AMOUNT, "ZbxSOL: below minimum");
        _spendAllowance(from, msg.sender, value);
        _burn(from, value); unchecked { _totalBurned += value; }
        emit Burn(from, value); return true;
    }
    function totalBurned() external view override returns (uint256) { return _totalBurned; }

    /// @notice Convert lamports to ZbxSOL (18-decimal).
    function lamportsToZbxSOL(uint256 lamports) external pure returns (uint256) {
        return lamports * 1e9;
    }

    function transferOwnership(address to) external onlyOwner {
        emit OwnershipTransferred(owner, to); owner = to;
    }
    function _owner() internal view override returns (address) { return owner; }
}