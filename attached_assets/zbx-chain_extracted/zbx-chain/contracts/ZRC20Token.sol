// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { ZRC20Base }         from "./ZRC20Base.sol";
import { IZRC20Mintable }    from "./interfaces/IZRC20Mintable.sol";
import { IZRC20Burnable }    from "./interfaces/IZRC20Burnable.sol";
import { IZRC20Freezable }   from "./interfaces/IZRC20Freezable.sol";
import { IZRC20Lockable }    from "./interfaces/IZRC20Lockable.sol";

/// @title ZRC20Token — General-purpose advanced ZRC-20 token (deploy via ZRC20Factory).
/// @notice Canonical user-deployable fungible token contract on Zebvix Chain.
///
/// @dev Features (as of S16-ZRC20-ADV):
///   - ZRC-20 base (ERC-20 + Permit + BatchTransfer + tokenInfo + logoURI update)
///   - Mintable (role-gated, with supply cap)
///   - Mint enable/disable: `pauseMinting` (toggleable) + `finalizeMinting` (one-way)
///   - Burnable (by holder or with allowance)
///   - Pausable transfers (owner emergency stop)
///   - Anti-bot: max transfer limit per tx (optional)
///   - Freezable (compliance/sanctions): owner can freeze accounts; frozen
///     accounts cannot send, receive, mint-to, or burn-from. USDC-style.
///   - Native time-lock per account (single active lock; growth-only updates;
///     auto-expires on `unlockTime`). Locked tokens stay in holder's wallet
///     but cannot be transferred until unlock.
///   - Initial supply minted in constructor (owner receives) — no separate
///     post-deploy mint() call needed (closes ZRC20Factory mint-revert bug).
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:standard   ZRC-20 v1.1

contract ZRC20Token is
    ZRC20Base,
    IZRC20Mintable,
    IZRC20Burnable,
    IZRC20Freezable,
    IZRC20Lockable
{

    // ─── S25-Y4 unchecked policy ─────────────────────────────────────────
    // All `unchecked { ... }` blocks in this contract fall into ONE of these
    // proven-safe categories (per S25 hardening pass):
    //   (a) post-require subtraction — preceding `require(x >= y)` proves
    //       `x - y` cannot underflow (token balance debits, allowance debits).
    //   (b) conservation pair — incrementing one slot by exactly the value
    //       just decremented (or vice-versa) from another, with the totalSupply
    //       invariant pre-checked (mint/burn/transfer leg of accounting pair).
    //   (c) bounded for-loop counter — `for (i; i < len; ) { ...; unchecked
    //       { i++; } }` where `len` is the bound; standard gas-saving pattern.
    //   (d) modular wrap intentional — uint32 timestamp/sequence wrap arithmetic
    //       (Uniswap V2 style); the wrap IS the spec.
    //   (e) UQ112x112 fixed-point shift — pre-bounded by uint112 reserve
    //       invariants (Uniswap V2 oracle accumulator).
    // Reviewers MUST classify any future `unchecked` block in this file
    // against one of (a)-(e) before merging; new categories require AUDIT entry.
    // ─────────────────────────────────────────────────────────────────────

    // ─── Access control ───────────────────────────────────────────────────

    address public owner;
    /// @notice Pending new owner — must call `acceptOwnership()` to complete the transfer.
    address public pendingOwner;
    mapping(address => bool) private _minters;

    // ─── Supply ───────────────────────────────────────────────────────────

    uint256 public override mintCap;
    uint256 private _totalBurned;

    // ─── Mint enable/disable (S16-ZRC20-ADV) ──────────────────────────────

    /// @notice Temporary pause flag — toggleable by owner.
    bool public mintingPaused;

    /// @notice One-way kill switch — once true, can NEVER be undone.
    ///         Use this to permanently make supply trustless (no future mints).
    bool public mintingFinalized;

    // ─── Pause (transfer-pause; distinct from mint-pause above) ───────────

    bool public paused;

    // ─── Anti-bot: max tx amount (0 = disabled) ───────────────────────────

    uint256 public maxTransferAmount;

    // ─── Freeze (S16-ZRC20-ADV) ───────────────────────────────────────────

    mapping(address => bool) private _frozen;

    // ─── Native lock (S16-ZRC20-ADV) ──────────────────────────────────────

    struct LockInfo {
        uint256 amount;
        uint64  unlockTime;
    }
    mapping(address => LockInfo) private _locks;

    // ─── Events ───────────────────────────────────────────────────────────

    event OwnershipTransferred(address indexed prev, address indexed next);
    /// @notice Emitted when a 2-step ownership transfer is initiated.
    event OwnershipTransferStarted(address indexed prev, address indexed next);
    event Paused(address by);
    event Unpaused(address by);
    event MaxTransferSet(uint256 amount);
    event MintingPausedToggled(bool isPaused, address indexed by);
    event MintingFinalizedEvent(address indexed by);

    // ─── Constructor ──────────────────────────────────────────────────────

    /// @param initialSupply_ Tokens minted to `owner_` at deployment. Must be
    ///                       ≤ resolved mintCap. Zero is allowed (mint later).
    constructor(
        string  memory name_,
        string  memory symbol_,
        uint8          decimals_,
        uint256        initialSupply_,
        uint256        mintCap_,
        string  memory logoURI_,
        address        owner_
    ) ZRC20Base(name_, symbol_, decimals_, logoURI_) {
        require(owner_ != address(0), "ZRC20Token: zero owner");

        owner   = owner_;
        mintCap = mintCap_ == 0 ? type(uint256).max : mintCap_;
        require(initialSupply_ <= mintCap, "ZRC20Token: initial > cap");

        _minters[owner_] = true;
        emit MinterAdded(owner_);

        if (initialSupply_ > 0) {
            _mint(owner_, initialSupply_);
            emit Mint(owner_, initialSupply_);
        }
    }

    // ─── Modifiers ────────────────────────────────────────────────────────

    modifier onlyOwner()    { require(msg.sender == owner,  "ZRC20Token: not owner");  _; }
    modifier onlyMinter()   { require(_minters[msg.sender], "ZRC20Token: not minter"); _; }
    modifier whenNotPaused(){ require(!paused,              "ZRC20Token: paused");     _; }

    // ─── IZRC20Mintable ───────────────────────────────────────────────────

    function mint(address to, uint256 value) external override onlyMinter returns (bool) {
        require(!mintingFinalized,                "ZRC20Token: minting finalized");
        require(!mintingPaused,                   "ZRC20Token: minting paused");
        require(totalSupply() + value <= mintCap, "ZRC20Token: cap exceeded");
        _mint(to, value);
        emit Mint(to, value);
        return true;
    }

    function isMinter(address a) external view override returns (bool) { return _minters[a]; }

    function addMinter(address a) external override onlyOwner {
        require(a != address(0), "ZRC20Token: zero address");
        _minters[a] = true;
        emit MinterAdded(a);
    }

    function removeMinter(address a) external override onlyOwner {
        _minters[a] = false;
        emit MinterRemoved(a);
    }

    // ─── Mint enable/disable (S16-ZRC20-ADV) ──────────────────────────────

    /// @notice Temporarily disable all minting. Reversible via `resumeMinting`.
    function pauseMinting() external onlyOwner {
        require(!mintingFinalized, "ZRC20Token: minting finalized");
        require(!mintingPaused,    "ZRC20Token: minting already paused");
        mintingPaused = true;
        emit MintingPausedToggled(true, msg.sender);
    }

    /// @notice Resume minting after a pause.
    function resumeMinting() external onlyOwner {
        require(!mintingFinalized, "ZRC20Token: minting finalized");
        require(mintingPaused,     "ZRC20Token: minting not paused");
        mintingPaused = false;
        emit MintingPausedToggled(false, msg.sender);
    }

    /// @notice PERMANENTLY disable minting. Cannot be undone.
    ///         Use to make the token's max supply trustless.
    function finalizeMinting() external onlyOwner {
        require(!mintingFinalized, "ZRC20Token: already finalized");
        mintingFinalized = true;
        emit MintingFinalizedEvent(msg.sender);
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

    // ─── Pause (transfer-pause) ───────────────────────────────────────────

    function pause()   external onlyOwner { paused = true;  emit Paused(msg.sender); }
    function unpause() external onlyOwner { paused = false; emit Unpaused(msg.sender); }

    // ─── Anti-bot ─────────────────────────────────────────────────────────

    function setMaxTransferAmount(uint256 amount) external onlyOwner {
        maxTransferAmount = amount;
        emit MaxTransferSet(amount);
    }

    // ─── IZRC20Freezable (S16-ZRC20-ADV) ──────────────────────────────────

    function freeze(address account) external override onlyOwner {
        require(account != address(0), "ZRC20Token: zero address");
        require(!_frozen[account],     "ZRC20Token: already frozen");
        _frozen[account] = true;
        emit Frozen(account, msg.sender);
    }

    function unfreeze(address account) external override onlyOwner {
        require(_frozen[account], "ZRC20Token: not frozen");
        _frozen[account] = false;
        emit Unfrozen(account, msg.sender);
    }

    function isFrozen(address account) external view override returns (bool) {
        return _frozen[account];
    }

    function frozenBalance(address account) external view override returns (uint256) {
        return _frozen[account] ? balanceOf(account) : 0;
    }

    // ─── IZRC20Lockable (S16-ZRC20-ADV) ───────────────────────────────────

    function lockTokens(address account, uint256 amount, uint64 unlockTime)
        external override onlyOwner
    {
        require(account != address(0),         "ZRC20Token: zero address");
        require(amount > 0,                    "ZRC20Token: zero amount");
        require(unlockTime > block.timestamp,  "ZRC20Token: unlock in past");
        require(balanceOf(account) >= amount,  "ZRC20Token: insufficient balance");

        LockInfo storage l = _locks[account];
        // Active lock = stored unlockTime in the future. Expired or never-locked
        // counts as "no active lock" and may be replaced freely.
        bool isActive = (l.amount > 0 && block.timestamp < l.unlockTime);
        require(!isActive, "ZRC20Token: active lock — use extendLock");

        l.amount     = amount;
        l.unlockTime = unlockTime;
        emit TokensLocked(account, amount, unlockTime);
    }

    function extendLock(address account, uint256 newAmount, uint64 newUnlockTime)
        external override onlyOwner
    {
        LockInfo storage l = _locks[account];
        require(l.amount > 0,                       "ZRC20Token: no lock");
        require(block.timestamp < l.unlockTime,     "ZRC20Token: lock expired");
        require(newAmount     >= l.amount,          "ZRC20Token: amount must grow");
        require(newUnlockTime >= l.unlockTime,      "ZRC20Token: time must grow");
        require(balanceOf(account) >= newAmount,    "ZRC20Token: insufficient balance");

        l.amount     = newAmount;
        l.unlockTime = newUnlockTime;
        emit LockExtended(account, newAmount, newUnlockTime);
    }

    function lockedBalanceOf(address account) public view override returns (uint256) {
        LockInfo memory l = _locks[account];
        if (l.amount == 0 || block.timestamp >= l.unlockTime) return 0;
        return l.amount;
    }

    function transferableBalance(address account) external view override returns (uint256) {
        uint256 bal    = balanceOf(account);
        uint256 locked = lockedBalanceOf(account);
        // Defensive: if a prior outflow ever drove balance < locked (cannot
        // happen via normal flows because _beforeTransfer enforces the
        // invariant), saturate to 0 rather than underflow.
        return bal > locked ? bal - locked : 0;
    }

    function lockInfo(address account)
        external view override returns (uint256 amount, uint64 unlockTime)
    {
        LockInfo memory l = _locks[account];
        return (l.amount, l.unlockTime);
    }

    // ─── Ownership (2-step — S-MED-01) ────────────────────────────────────

    /// @notice Begin a 2-step ownership transfer.
    ///         The new owner must call `acceptOwnership()` to finalise.
    ///         Prevents accidental transfers to wrong addresses.
    function transferOwnership(address newOwner) external onlyOwner {
        require(newOwner != address(0), "ZRC20Token: zero address");
        pendingOwner = newOwner;
        emit OwnershipTransferStarted(owner, newOwner);
    }

    /// @notice Complete the 2-step ownership transfer. Only callable by `pendingOwner`.
    function acceptOwnership() external {
        require(msg.sender == pendingOwner, "ZRC20Token: not pending owner");
        emit OwnershipTransferred(owner, pendingOwner);
        owner        = pendingOwner;
        pendingOwner = address(0);
    }

    function renounceOwnership() external onlyOwner {
        emit OwnershipTransferred(owner, address(0));
        owner = address(0);
    }

    /// @notice Update the on-chain token logo URI (calls `_setLogoURI` in base).
    function updateLogoURI(string calldata newURI) external onlyOwner {
        _setLogoURI(newURI);
    }

    // ─── ZRC20Base Overrides ──────────────────────────────────────────────

    function _owner() internal view override returns (address) { return owner; }

    /// @dev Combined hook: pause + freeze + lock + anti-bot.
    ///      Order matters — cheapest checks first to fail fast.
    function _beforeTransfer(address from, address to, uint256 value)
        internal override whenNotPaused
    {
        // Freeze: applies to mint-to (from=0), burn-from (to=0), and transfers.
        // address(0) can never be frozen (require in `freeze`), so the zero
        // sentinel branch is implicitly a no-op.
        require(!_frozen[from], "ZRC20Token: from frozen");
        require(!_frozen[to],   "ZRC20Token: to frozen");

        // Native lock: only meaningful on outgoing (skip mint where from = 0).
        if (from != address(0)) {
            uint256 locked = lockedBalanceOf(from);
            if (locked > 0) {
                uint256 bal = balanceOf(from);
                // bal is pre-transfer; need value <= bal - locked.
                // Rearranged to dodge underflow when value > bal (caught later
                // by base's `insufficient balance` revert anyway, but this
                // gives a more specific error first).
                require(bal >= locked && bal - locked >= value,
                        "ZRC20Token: tokens locked");
            }
        }

        // Anti-bot (only peer-to-peer transfers; mint and burn exempt).
        if (maxTransferAmount > 0 && from != address(0) && to != address(0)) {
            require(value <= maxTransferAmount, "ZRC20Token: exceeds max transfer");
        }
    }

    // ─── EIP-165 supportsInterface (S21) ───────────────────────────────────
    //
    // Inherits the IZRC20 + IERC165 claim from ZRC20Base via super, then
    // OR-s in this contract's four extension interfaces:
    //   - IZRC20Mintable  — role-gated mint with cap
    //   - IZRC20Burnable  — burn / burnFrom
    //   - IZRC20Freezable — sanctions/compliance freeze
    //   - IZRC20Lockable  — per-account time-lock
    function supportsInterface(bytes4 interfaceId) public pure override returns (bool) {
        return super.supportsInterface(interfaceId)
            || interfaceId == type(IZRC20Mintable).interfaceId
            || interfaceId == type(IZRC20Burnable).interfaceId
            || interfaceId == type(IZRC20Freezable).interfaceId
            || interfaceId == type(IZRC20Lockable).interfaceId;
    }
}
