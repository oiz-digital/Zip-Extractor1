// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { ZRC20Base }       from "./ZRC20Base.sol";
import { IZRC20Mintable }  from "./interfaces/IZRC20Mintable.sol";
import { IZRC20Burnable }  from "./interfaces/IZRC20Burnable.sol";
import { IZRC20Freezable } from "./interfaces/IZRC20Freezable.sol";

/// @title ZRC20 — Wrapped Zebvix (ZBX) on external chains (BNB, Polygon, etc.)
/// @notice Canonical bridge-wrapped representation of native Zebvix L1 ZBX.
///         Minted by BridgeVault when ZBX is locked on Zebvix Chain.
///         Burned by BridgeVault when user bridges back to Zebvix Chain.
///
///         Supply invariant:
///           totalSupply() on this chain ≤ ZBX locked in BridgeVault on Zebvix Chain
///
/// @custom:zbx-chain  Chain ID 8989 (native) / multi-chain (wrapped)
/// @custom:standard   ZRC-20 v1.0
/// @custom:bridge     BridgeVault.sol
/// @custom:freeze     S20 — sanctions/compliance parity with ZRC20Token (S16-ZRC20-ADV).
///                    A wallet sanctioned on Zebvix Chain mainnet ZRC20Token MUST
///                    also be freezable on the wrapped-ZBX side; otherwise the
///                    bridge becomes a sanctions-laundering vector. Freeze is
///                    enforced at the `_beforeTransfer` hook so ALL movement
///                    (transfer, transferFrom, mint, burn, batch) is blocked
///                    while frozen — exact parity with ZRC20Token.

contract ZRC20 is ZRC20Base, IZRC20Mintable, IZRC20Burnable, IZRC20Freezable {

    // ─── Roles ────────────────────────────────────────────────────────────

    address public owner;
    /// @notice Pending new owner — must call `acceptOwnership()` to complete transfer.
    address public pendingOwner;
    mapping(address => bool) private _minters;
    uint256 public override mintCap;   // maximum mintable supply

    // ─── Burn tracking ────────────────────────────────────────────────────

    uint256 private _totalBurned;

    // ─── Freeze (S20 — IZRC20Freezable parity with ZRC20Token) ────────────
    //
    // Per-account freeze flag. Set by `freeze()` (owner-only), cleared by
    // `unfreeze()`. Enforced at the `_beforeTransfer` hook so every code
    // path that moves balance (transfer, transferFrom, batch, mint, burn)
    // is blocked while either side is frozen.
    //
    // Invariants (mirrored from ZRC20Token):
    //   - address(0) is NEVER stored as frozen (the freeze() require gate).
    //     This keeps `mint(0→to)` and `burn(from→0)` working while the
    //     non-zero counter-party is unfrozen, and means the
    //     "from frozen" / "to frozen" checks below are zero-safe even
    //     though _beforeTransfer is invoked with address(0) on mint/burn.
    //   - Freeze state is orthogonal to mint cap, burn-tracking, and the
    //     bridge supply invariant. Freezing does NOT alter totalSupply().
    mapping(address => bool) private _frozen;

    // ─── Events ───────────────────────────────────────────────────────────

    event OwnershipTransferred(address indexed previousOwner, address indexed newOwner);
    /// @notice Emitted when a 2-step ownership transfer is initiated.
    event OwnershipTransferStarted(address indexed previousOwner, address indexed newOwner);

    // ─── Constructor ──────────────────────────────────────────────────────

    /// @param mintCap_   Maximum tokens that can ever be minted (0 = unlimited).
    ///                   For the canonical wrapped ZBX, this equals 150,000,000 × 1e18.
    constructor(uint256 mintCap_) ZRC20Base(
        "Zebvix",
        "ZBX",
        18,
        "ipfs://QmZebvixLogoXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX"
    ) {
        owner    = msg.sender;
        mintCap  = mintCap_ == 0 ? type(uint256).max : mintCap_;
        _minters[msg.sender] = true;
    }

    // ─── Modifiers ────────────────────────────────────────────────────────

    modifier onlyOwner() {
        require(msg.sender == owner, "ZRC20: not owner");
        _;
    }

    modifier onlyMinter() {
        require(_minters[msg.sender], "ZRC20: not minter");
        _;
    }

    // ─── IZRC20Mintable ───────────────────────────────────────────────────

    function mint(address to, uint256 value) external override onlyMinter returns (bool) {
        require(totalSupply() + value <= mintCap, "ZRC20: mint cap exceeded");
        _mint(to, value);
        emit Mint(to, value);
        return true;
    }

    function isMinter(address account) external view override returns (bool) {
        return _minters[account];
    }

    function addMinter(address account) external override onlyOwner {
        require(account != address(0), "ZRC20: zero address");
        _minters[account] = true;
        emit MinterAdded(account);
    }

    function removeMinter(address account) external override onlyOwner {
        _minters[account] = false;
        emit MinterRemoved(account);
    }

    function updateMintCap(uint256 newCap) external onlyOwner {
        require(newCap >= totalSupply(), "ZRC20: cap below current supply");
        emit MintCapUpdated(mintCap, newCap);
        mintCap = newCap;
    }

    // ─── IZRC20Burnable ───────────────────────────────────────────────────

    function burn(uint256 value) external override returns (bool) {
        _burn(msg.sender, value);
        _totalBurned += value;
        emit Burn(msg.sender, value);
        return true;
    }

    function burnFrom(address from, uint256 value) external override returns (bool) {
        _spendAllowance(from, msg.sender, value);
        _burn(from, value);
        _totalBurned += value;
        emit Burn(from, value);
        return true;
    }

    function totalBurned() external view override returns (uint256) { return _totalBurned; }

    // ─── Admin ────────────────────────────────────────────────────────────

    // ─── Ownership (2-step — S-MED-01) ────────────────────────────────────

    /// @notice Begin a 2-step ownership transfer.
    ///         The new owner must call `acceptOwnership()` to finalise.
    ///         Prevents accidental permanent lock from a wrong address.
    function transferOwnership(address newOwner) external onlyOwner {
        require(newOwner != address(0), "ZRC20: zero address");
        pendingOwner = newOwner;
        emit OwnershipTransferStarted(owner, newOwner);
    }

    /// @notice Complete the 2-step ownership transfer. Only callable by `pendingOwner`.
    function acceptOwnership() external {
        require(msg.sender == pendingOwner, "ZRC20: not pending owner");
        emit OwnershipTransferred(owner, pendingOwner);
        owner        = pendingOwner;
        pendingOwner = address(0);
    }

    /// @notice Update the on-chain token logo URI. Emits `LogoURIUpdated`.
    /// @dev    Previously a silent no-op; now persists via `_setLogoURI`
    ///         (added to ZRC20Base in S16-ZRC20-ADV).
    function updateLogoURI(string calldata newURI) external onlyOwner {
        _setLogoURI(newURI);
    }

    // ─── IZRC20Freezable (S20 — bridge sanctions/compliance parity) ───────

    /// @notice Freeze `account`, blocking all inbound and outbound movement
    ///         (incl. mint to / burn from this address) until unfrozen.
    /// @dev    Owner-only. Mirrors `ZRC20Token.freeze` byte-for-byte so
    ///         off-chain compliance pipelines can reuse the same revert
    ///         strings on either side of the bridge.
    function freeze(address account) external override onlyOwner {
        require(account != address(0), "ZRC20: zero address");
        require(!_frozen[account],     "ZRC20: already frozen");
        _frozen[account] = true;
        emit Frozen(account, msg.sender);
    }

    /// @notice Lift the freeze on `account`. Owner-only.
    function unfreeze(address account) external override onlyOwner {
        require(_frozen[account], "ZRC20: not frozen");
        _frozen[account] = false;
        emit Unfrozen(account, msg.sender);
    }

    /// @notice Whether `account` is currently frozen.
    function isFrozen(address account) external view override returns (bool) {
        return _frozen[account];
    }

    /// @notice Returns the account's full balance if frozen, else 0.
    ///         Useful for off-chain compliance dashboards.
    function frozenBalance(address account) external view override returns (uint256) {
        return _frozen[account] ? balanceOf(account) : 0;
    }

    // ─── ZRC20Base Overrides ──────────────────────────────────────────────

    function _owner() internal view override returns (address) { return owner; }

    /// @dev S20 freeze enforcement. Invoked on transfer, transferFrom, batch,
    ///      mint (`from = address(0)`), and burn (`to = address(0)`).
    ///      The freeze() require-gate guarantees address(0) is never in
    ///      `_frozen`, so the checks below are zero-safe. The `value`
    ///      parameter is intentionally unused (no anti-bot / lock policy
    ///      on wrapped ZBX — those live on the L1 ZRC20Token); declared
    ///      unnamed in the signature for warning-clean builds.
    function _beforeTransfer(address from, address to, uint256 /* value */)
        internal override
    {
        require(!_frozen[from], "ZRC20: from frozen");
        require(!_frozen[to],   "ZRC20: to frozen");
    }

    // ─── supportsInterface (EIP-165) ──────────────────────────────────────
    //
    // S21: now extends ZRC20Base.supportsInterface (which itself claims
    // IZRC20 + IERC165). The OR-chain below adds the bridge-wrapped
    // contract's three extension interfaceIds. Visibility upgraded
    // `external` → `public` so the parent dispatch via `super.` is allowed.
    function supportsInterface(bytes4 interfaceId) public pure override returns (bool) {
        return super.supportsInterface(interfaceId)
            || interfaceId == type(IZRC20Mintable).interfaceId
            || interfaceId == type(IZRC20Burnable).interfaceId
            || interfaceId == type(IZRC20Freezable).interfaceId;
    }
}