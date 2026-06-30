// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { IZRC20 } from "./interfaces/IZRC20.sol";

/// @title ZRC20Base — Reference implementation of the ZRC-20 token standard.
/// @notice Deploy this contract (or inherit from it) to create a ZRC-20 token
///         on Zebvix Chain (Chain ID 8989 mainnet / 8990 testnet+devnet).
///
/// @dev Security model:
///   - Owner-only minting (override `_mint` access in subcontracts).
///   - EIP-2612 permit uses EIP-712 typed signatures (replay-protected by nonce + deadline).
///   - Batch transfer reverts the entire tx if any single transfer fails.
///   - No transfer tax or fee hooks (use `_beforeTransfer` / `_afterTransfer` to add).
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:standard   ZRC-20 v1.0
/// @custom:audits     Pending — not for mainnet use without audit.

abstract contract ZRC20Base is IZRC20 {

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

    // ─── Storage ───────────────────────────────────────────────────────────

    string  private _name;
    string  private _symbol;
    uint8   private immutable _decimals;
    string  private _logoURI;

    uint256 private _totalSupply;

    mapping(address => uint256) private _balances;
    mapping(address => mapping(address => uint256)) private _allowances;

    // EIP-2612 Permit
    mapping(address => uint256) private _nonces;
    bytes32 private immutable _domainSeparator;

    // ─── EIP-712 Type Hashes ───────────────────────────────────────────────

    bytes32 private constant _PERMIT_TYPEHASH = keccak256(
        "Permit(address owner,address spender,uint256 value,uint256 nonce,uint256 deadline)"
    );

    // ─── Constructor ───────────────────────────────────────────────────────

    constructor(
        string memory name_,
        string memory symbol_,
        uint8         decimals_,
        string memory logoURI_
    ) {
        _name     = name_;
        _symbol   = symbol_;
        _decimals = decimals_;
        _logoURI  = logoURI_;

        _domainSeparator = _buildDomainSeparator();
    }

    // ─── IZRC20 Core ────────────────────────────────────────────────────────

    function name()        public view virtual override returns (string memory) { return _name; }
    function symbol()      public view virtual override returns (string memory) { return _symbol; }
    function decimals()    public view virtual override returns (uint8)         { return _decimals; }
    function totalSupply() public view virtual override returns (uint256)       { return _totalSupply; }

    function balanceOf(address account) public view virtual override returns (uint256) {
        return _balances[account];
    }

    function allowance(address owner, address spender) public view virtual override returns (uint256) {
        return _allowances[owner][spender];
    }

    function transfer(address to, uint256 value) public virtual override returns (bool) {
        _transfer(msg.sender, to, value);
        return true;
    }

    function approve(address spender, uint256 value) public virtual override returns (bool) {
        _approve(msg.sender, spender, value);
        return true;
    }

    function transferFrom(address from, address to, uint256 value) public virtual override returns (bool) {
        _spendAllowance(from, msg.sender, value);
        _transfer(from, to, value);
        return true;
    }

    // ─── ZRC-20 Extension: Batch Transfer ──────────────────────────────────

    function batchTransfer(
        address[] calldata to,
        uint256[] calldata values
    ) external virtual override returns (bool) {
        require(to.length == values.length, "ZRC20: length mismatch");
        require(to.length > 0,              "ZRC20: empty batch");
        require(to.length <= 512,           "ZRC20: batch too large (max 512)");

        // Per-leg routing through `_transfer` so every batched movement runs
        // through `_beforeTransfer` / `_afterTransfer` hooks. (Closes
        // S16-ZRC20-ADV CRIT-1: prior bulk-debit implementation bypassed
        // freeze, native lock, transfer-pause, and anti-bot guards in any
        // ZRC20Base subclass that overrode the hooks.) The per-leg lock
        // check works correctly under serial debits — see ZEP-006 §5.2.
        for (uint256 i; i < to.length; ++i) {
            _transfer(msg.sender, to[i], values[i]);
        }

        emit BatchTransfer(msg.sender, to, values);
        return true;
    }

    // ─── ZRC-20 Extension: EIP-2612 Permit ─────────────────────────────────

    function permit(
        address owner,
        address spender,
        uint256 value,
        uint256 deadline,
        uint8   v,
        bytes32 r,
        bytes32 s
    ) external virtual override {
        require(block.timestamp <= deadline, "ZRC20Permit: expired");

        bytes32 structHash = keccak256(abi.encode(
            _PERMIT_TYPEHASH,
            owner,
            spender,
            value,
            _nonces[owner]++,
            deadline
        ));
        bytes32 hash = _toTypedDataHash(_domainSeparator, structHash);
        address recovered = ecrecover(hash, v, r, s);
        require(recovered != address(0) && recovered == owner, "ZRC20Permit: invalid signature");

        _approve(owner, spender, value);
        emit Permit(owner, spender, value, deadline);
    }

    function DOMAIN_SEPARATOR() external view override returns (bytes32) { return _domainSeparator; }
    function nonces(address owner) external view override returns (uint256) { return _nonces[owner]; }

    // ─── ZRC-20 Extension: Metadata ────────────────────────────────────────

    function logoURI() external view virtual override returns (string memory) { return _logoURI; }

    function tokenInfo() external view virtual override returns (
        string memory tokenName,
        string memory tokenSymbol,
        uint8         tokenDecimals,
        uint256       supply,
        address       tokenOwner,
        string memory logo
    ) {
        return (_name, _symbol, _decimals, _totalSupply, _owner(), _logoURI);
    }

    // ─── Internal Helpers ───────────────────────────────────────────────────

    function _transfer(address from, address to, uint256 value) internal virtual {
        require(from != address(0), "ZRC20: transfer from zero");
        require(to   != address(0), "ZRC20: transfer to zero");

        _beforeTransfer(from, to, value);

        uint256 fromBalance = _balances[from];
        require(fromBalance >= value, "ZRC20: insufficient balance");
        unchecked { _balances[from] = fromBalance - value; }
        unchecked { _balances[to]  += value; }

        emit Transfer(from, to, value);
        _afterTransfer(from, to, value);
    }

    function _mint(address to, uint256 value) internal virtual {
        require(to != address(0), "ZRC20: mint to zero");
        // Hook coverage on mint (closes S16-ZRC20-ADV CRIT-2): subclasses
        // can enforce freeze (block mint-to-frozen), pause (block mint
        // during emergency pause), and any other policy via _beforeTransfer.
        // Lock check on mint is a no-op because the hook in ZRC20Token
        // skips when `from == address(0)`.
        _beforeTransfer(address(0), to, value);
        _totalSupply += value;
        unchecked { _balances[to] += value; }
        emit Transfer(address(0), to, value);
        _afterTransfer(address(0), to, value);
    }

    function _burn(address from, uint256 value) internal virtual {
        require(from != address(0), "ZRC20: burn from zero");
        // Hook coverage on burn (closes S16-ZRC20-ADV CRIT-2): freeze blocks
        // burn-from-frozen (USDC compliance), pause blocks burn during
        // emergency, and lock blocks burning the locked portion (prevents a
        // holder from bypassing a time-lock by burning + recovering via
        // governance/airdrop). See ZEP-006 §3.2 lock-on-burn rationale.
        _beforeTransfer(from, address(0), value);
        uint256 fromBalance = _balances[from];
        require(fromBalance >= value, "ZRC20: burn exceeds balance");
        unchecked {
            _balances[from] = fromBalance - value;
            _totalSupply   -= value;
        }
        emit Transfer(from, address(0), value);
        _afterTransfer(from, address(0), value);
    }

    function _approve(address owner, address spender, uint256 value) internal virtual {
        require(owner   != address(0), "ZRC20: approve from zero");
        require(spender != address(0), "ZRC20: approve to zero");
        _allowances[owner][spender] = value;
        emit Approval(owner, spender, value);
    }

    function _spendAllowance(address owner, address spender, uint256 value) internal virtual {
        uint256 current = _allowances[owner][spender];
        if (current != type(uint256).max) {
            require(current >= value, "ZRC20: insufficient allowance");
            unchecked { _allowances[owner][spender] = current - value; }
        }
    }

    /// @dev Override point: called before every transfer/mint/burn.
    function _beforeTransfer(address from, address to, uint256 value) internal virtual {}

    /// @dev Override point: called after every transfer/mint/burn.
    function _afterTransfer(address from, address to, uint256 value) internal virtual {}

    /// @dev Override to return contract owner (for tokenInfo).
    function _owner() internal view virtual returns (address) { return address(0); }

    /// @dev Internal logo URI setter — exposed via subcontract admin functions.
    ///      Emits `LogoURIUpdated(old, new)`. Subcontracts MUST gate their
    ///      external wrapper with appropriate access control (e.g. `onlyOwner`).
    function _setLogoURI(string memory newURI) internal virtual {
        string memory old = _logoURI;
        _logoURI = newURI;
        emit LogoURIUpdated(old, newURI);
    }

    // ─── EIP-712 Helpers ───────────────────────────────────────────────────

    function _buildDomainSeparator() private view returns (bytes32) {
        return keccak256(abi.encode(
            keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"),
            keccak256(bytes(_name)),
            keccak256(bytes("1")),
            block.chainid,
            address(this)
        ));
    }

    function _toTypedDataHash(bytes32 domainSep, bytes32 structHash) private pure returns (bytes32) {
        // EIP-712: digest = keccak256(0x19 || 0x01 || domainSeparator || structHash).
        // Earlier the literal "\\x19\\x01" stored a 6-char string instead of two
        // bytes, breaking permit() / EIP-2612 signatures (which cascaded into
        // BridgeVault.lockWithPermit). Use bytes1 literals so the prefix is
        // exactly two bytes 0x19 0x01.
        return keccak256(abi.encodePacked(bytes1(0x19), bytes1(0x01), domainSep, structHash));
    }

    // ─── EIP-165 supportsInterface (S21) ───────────────────────────────────
    //
    // Base claim: this contract implements the IZRC20 interface and EIP-165
    // itself. Subclasses (ZRC20.sol bridge wrapper, ZRC20Token general-purpose)
    // override this and OR-in their own extension interfaceIds via
    // `super.supportsInterface(interfaceId) || ...`. This keeps the Base
    // surface the canonical source of truth for the IZRC20 claim.
    //
    // Declared `public virtual` (not `external`) so subclasses can use
    // `super.supportsInterface(...)` to chain into this base implementation.
    function supportsInterface(bytes4 interfaceId) public pure virtual returns (bool) {
        return interfaceId == type(IZRC20).interfaceId
            || interfaceId == 0x01ffc9a7;   // EIP-165 itself
    }
}