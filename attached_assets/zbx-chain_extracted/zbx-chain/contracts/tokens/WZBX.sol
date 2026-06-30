// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title WZBX — Wrapped Zebvix
/// @notice Converts native ZBX (gas token) into a ZRC-20 compatible token.
///         Identical in design to WETH on Ethereum.
///         Needed because native ZBX cannot be used directly in DeFi contracts
///         that expect a ZRC-20 interface (AMMs, lending pools, etc.).
///
/// @dev  Invariant: address(this).balance == totalSupply() always.
///       Deposit:  send ZBX → receive WZBX 1:1
///       Withdraw: burn WZBX → receive ZBX 1:1
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:ticker     WZBX
/// @custom:decimals   18

contract WZBX {

    // ─── ERC-20 metadata ──────────────────────────────────────────────────

    string  public constant name     = "Wrapped Zebvix";
    string  public constant symbol   = "WZBX";
    uint8   public constant decimals = 18;

    // ─── Events ───────────────────────────────────────────────────────────

    event Transfer(address indexed from,  address indexed to,      uint256 value);
    event Approval(address indexed owner, address indexed spender, uint256 value);
    event Deposit( address indexed from,  uint256 value);
    event Withdrawal(address indexed to,  uint256 value);

    // ─── State ────────────────────────────────────────────────────────────

    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    // ─── Deposit / Withdraw ───────────────────────────────────────────────

    /// @notice Wrap native ZBX → WZBX. Call with msg.value > 0.
    function deposit() public payable {
        balanceOf[msg.sender] += msg.value;
        emit Deposit(msg.sender, msg.value);
        emit Transfer(address(0), msg.sender, msg.value);
    }

    /// @notice Unwrap WZBX → native ZBX.
    /// @dev S-LOW-01: use `.call{value:}()` instead of `.transfer()` — the
    ///      2300-gas stipend of `.transfer()` fails for smart-contract callers
    ///      (ERC-4337 accounts, multi-sigs) under EIP-2929 cold-account costs.
    function withdraw(uint256 value) external {
        require(balanceOf[msg.sender] >= value, "WZBX: insufficient balance");
        balanceOf[msg.sender] -= value;
        emit Withdrawal(msg.sender, value);
        emit Transfer(msg.sender, address(0), value);
        (bool ok, ) = payable(msg.sender).call{value: value}("");
        require(ok, "WZBX: ETH transfer failed");
    }

    // ─── Fallback ─────────────────────────────────────────────────────────

    receive()  external payable { deposit(); }
    fallback() external payable { deposit(); }

    // ─── ZRC-20 Core ──────────────────────────────────────────────────────

    function totalSupply() external view returns (uint256) {
        return address(this).balance;
    }

    function approve(address spender, uint256 value) external returns (bool) {
        allowance[msg.sender][spender] = value;
        emit Approval(msg.sender, spender, value);
        return true;
    }

    function transfer(address to, uint256 value) external returns (bool) {
        return transferFrom(msg.sender, to, value);
    }

    function transferFrom(address from, address to, uint256 value) public returns (bool) {
        require(balanceOf[from] >= value, "WZBX: insufficient balance");
        if (from != msg.sender && allowance[from][msg.sender] != type(uint256).max) {
            require(allowance[from][msg.sender] >= value, "WZBX: insufficient allowance");
            allowance[from][msg.sender] -= value;
        }
        balanceOf[from] -= value;
        balanceOf[to]   += value;
        emit Transfer(from, to, value);
        return true;
    }
}