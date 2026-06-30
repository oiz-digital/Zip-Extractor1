// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title ZbxPaymaster — ERC-4337 Paymaster (gas sponsorship).
/// @notice Allows third parties to pay ZBX gas fees on behalf of users.
///         Use cases:
///           - DApp operators pay gas for their users (gasless UX)
///           - Pay gas in ZRC-20 tokens (not native ZBX)
///           - Subscription-based gas (pay monthly, use freely)
///           - Promotional gas credits
///
/// @dev   Paymaster policies (this contract implements "Verifying Paymaster"):
///           - Operator signs a permit off-chain.
///           - User includes the signed permit in UserOperation.paymasterAndData.
///           - Paymaster verifies the signature and pays for gas.
///
/// @custom:zbx-chain  Chain ID 8989
///
/// @custom:audit-2026-04-30  S4-A2 (HIGH) closed:
///   The validate flow now receives `user` (op.sender) explicitly and rejects
///   any address present in `blocked[]`. Previously the blocklist existed only
///   as state; nothing read it. Bonus: `PermitUsed` now logs the real user.

contract ZbxPaymaster {

    address public entryPoint;
    address public owner;
    address public signer;      // signs gas permits off-chain

    uint256 public deposit;     // ZBX deposited in EntryPoint

    mapping(address => bool)    public blocked;         // blocked users
    mapping(bytes32 => bool)    public usedPermits;     // prevent replay

    event GasSponsored(address indexed user, uint256 gasCost, address sponsor);
    event PermitUsed(bytes32 indexed permitHash, address indexed user);
    event Blocked(address indexed user);
    event Unblocked(address indexed user);

    constructor(address entryPoint_, address signer_) {
        require(entryPoint_ != address(0), "Paymaster: entryPoint=0");
        require(signer_     != address(0), "Paymaster: signer=0");
        entryPoint = entryPoint_;
        owner      = msg.sender;
        signer     = signer_;
    }

    modifier onlyOwner()      { require(msg.sender == owner,      "Paymaster: not owner"); _; }
    modifier onlyEntryPoint() { require(msg.sender == entryPoint, "Paymaster: not entryPoint"); _; }

    // ─── Paymaster validation (called by EntryPoint) ──────────────────────

    /// @notice Validate a UserOperation's paymaster data.
    /// @param  user           op.sender — the smart wallet whose op is being sponsored.
    /// @param  userOpHash     Hash of the UserOperation.
    /// @param  maxCost        Maximum gas cost for this operation.
    /// @param  paymasterData  abi.encode(deadline, signature)
    /// @return context        Opaque blob handed back to postOp().
    /// @return validationData 0 = success (per ERC-4337 v0.6).
    function validatePaymasterUserOp(
        address user,
        bytes32 userOpHash,
        uint256 maxCost,
        bytes calldata paymasterData
    ) external onlyEntryPoint returns (bytes memory context, uint256 validationData) {
        // Blocklist enforcement: any address marked `blocked` is rejected
        // BEFORE we touch the signature, so a blocked attacker cannot even
        // burn paymaster gas trying to spam permits.
        require(!blocked[user], "Paymaster: user blocked");

        // paymasterData = abi.encode(deadline, signature)
        (uint256 deadline, bytes memory sig) = abi.decode(paymasterData, (uint256, bytes));
        require(block.timestamp <= deadline, "Paymaster: permit expired");

        // Verify signer's signature over (userOpHash, deadline, maxCost, user).
        // Including `user` in the digest binds the permit to the actual sender,
        // so a stolen permit cannot be replayed against a different address.
        bytes32 permitHash = keccak256(
            abi.encodePacked(userOpHash, deadline, maxCost, user)
        );
        require(!usedPermits[permitHash], "Paymaster: permit already used");
        require(_verifySignature(permitHash, sig), "Paymaster: invalid signature");

        usedPermits[permitHash] = true;
        emit PermitUsed(permitHash, user);

        // Carry permitHash + user + maxCost into postOp so the post-call
        // bookkeeping can attribute the GasSponsored event correctly.
        context = abi.encode(permitHash, user, maxCost);
        validationData = 0; // success
    }

    /// @notice Called after UserOperation execution to charge the paymaster.
    function postOp(
        uint8 mode,           // 0 = success, 1 = reverted (per ERC-4337)
        bytes calldata context,
        uint256 actualGasCost
    ) external onlyEntryPoint {
        (, address user, ) = abi.decode(context, (bytes32, address, uint256));
        // The paymaster has already been debited via the EntryPoint deposit.
        // We only need to log who consumed the sponsorship and how much.
        // `mode` is currently informational; future variants may refund the
        // user on revert (mode == 1) when sponsoring a fee-free trial flow.
        mode; // silence unused-param warning while preserving the ABI
        emit GasSponsored(user, actualGasCost, owner);
    }

    // ─── Token paymaster variant ─────────────────────────────────────────

    /// @notice Allow users to pay gas in ZRC-20 tokens instead of native ZBX.
    ///         Accepts payment in USDT/USDC at oracle price, converts to ZBX.
    function validateTokenPayment(
        address token,
        uint256 tokenAmount,
        address user,
        uint256 maxGasCostZBX
    ) external view returns (bool) {
        // Real impl: query ZbxOracle for token/ZBX rate,
        // check user's token allowance and balance.
        require(!blocked[user], "Paymaster: user blocked");
        return tokenAmount > 0 && maxGasCostZBX > 0 && token != address(0);
    }

    // ─── Deposit management ────────────────────────────────────────────────

    function depositToEntryPoint() external payable onlyOwner {
        deposit += msg.value;
        (bool ok, ) = entryPoint.call{value: msg.value}(
            abi.encodeWithSignature("depositTo(address)", address(this))
        );
        require(ok, "Paymaster: deposit failed");
    }

    function withdrawFromEntryPoint(uint256 amount) external onlyOwner {
        deposit -= amount;
        (bool ok, ) = entryPoint.call(
            abi.encodeWithSignature("withdrawTo(address,uint256)", payable(owner), amount)
        );
        require(ok, "Paymaster: withdrawal failed");
    }

    // ─── Admin ────────────────────────────────────────────────────────────

    function blockUser(address user) external onlyOwner {
        blocked[user] = true;
        emit Blocked(user);
    }

    function unblockUser(address user) external onlyOwner {
        blocked[user] = false;
        emit Unblocked(user);
    }

    function setSigner(address newSigner) external onlyOwner {
        require(newSigner != address(0), "Paymaster: signer=0");
        signer = newSigner;
    }

    // ─── Internal ─────────────────────────────────────────────────────────

    function _verifySignature(bytes32 hash, bytes memory sig) internal view returns (bool) {
        if (sig.length != 65) return false;
        bytes32 r; bytes32 s; uint8 v;
        // S25-Y3 assembly: split fixed-65-byte ECDSA signature into (r,s,v) from
        // memory bytes. `mload(add(sig,32))` skips the 32-byte length prefix.
        // Bounds proven by `sig.length != 65 → return` on the previous line.
        assembly { r := mload(add(sig, 32)) s := mload(add(sig, 64)) v := byte(0, mload(add(sig, 96))) }
        address recovered = ecrecover(
            keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", hash)),
            v, r, s
        );
        return recovered != address(0) && recovered == signer;
    }

    receive() external payable {}
}
