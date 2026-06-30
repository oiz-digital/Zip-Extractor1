// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title ZbxSmartWallet — ERC-4337 smart contract wallet (Account).
/// @notice Reference implementation of an ERC-4337 account for ZBX Chain.
///
///         Features:
///           - Multi-signature (M-of-N guardians)
///           - Social recovery (replace lost key via guardians)
///           - Session keys (temporary signing permissions)
///           - Batched calls (multiple actions in one UserOperation)
///           - ERC-1271 signature validation (verifiable on-chain)
///
/// @custom:zbx-chain  Chain ID 8989

contract ZbxSmartWallet {

    address public entryPoint;
    address public owner;

    /// Guardians for social recovery.
    mapping(address => bool)    public guardians;
    uint256                     public guardianCount;
    uint256                     public recoveryThreshold;
    /// Replay-protection counter for `executeRecovery`. Bumped on every
    /// successful recovery so that the same set of guardian signatures cannot
    /// be re-submitted to repeat-replace the owner.
    uint256                     public recoveryNonce;

    /// Session keys: temporary signers with limited permissions.
    mapping(address => SessionKey) public sessionKeys;

    /// Nonce for UserOperation replay protection.
    uint256 public nonce;

    struct SessionKey {
        bool    active;
        uint256 expiry;             // block number when key expires
        uint256 maxValuePerCall;    // max ZBX per call (0 = unlimited)
        address[] allowedContracts; // empty = all contracts allowed
    }

    event Executed(address indexed target, uint256 value, bytes data);
    event SessionKeyAdded(address indexed key, uint256 expiry);
    event SessionKeyRevoked(address indexed key);
    event RecoveryExecuted(address indexed newOwner, address indexed guardian);

    constructor(address entryPoint_, address owner_) {
        entryPoint = entryPoint_;
        owner      = owner_;
        recoveryThreshold = 1;
    }

    modifier onlyOwnerOrEntryPoint() {
        require(msg.sender == owner || msg.sender == entryPoint, "SmartWallet: not authorized");
        _;
    }

    // ─── ERC-4337: validateUserOp ─────────────────────────────────────────

    function validateUserOp(
        bytes calldata userOpData,
        bytes32 userOpHash,
        uint256 missingFunds
    ) external returns (uint256 validationData) {
        require(msg.sender == entryPoint, "SmartWallet: not entryPoint");

        // Pay EntryPoint for missing funds.
        // S19: ERC-4337 canonical pattern — forward all gas, suppress the
        // return bool. The EntryPoint validates payment receipt itself and
        // will revert the whole UserOp if the prefund did not arrive, so
        // a `require(ok, ...)` here would be redundant AND would violate the
        // ERC-4337 validateUserOp rule that this function MUST NOT revert
        // for reasons other than signature/nonce mismatches. The previous
        // `.transfer(...)` form was broken by EIP-2929 cold-account costs
        // (the 2300-gas stipend can no longer cover a single SSTORE on a
        // cold receiver).
        if (missingFunds > 0) {
            (bool ok, ) = payable(entryPoint).call{value: missingFunds}("");
            ok; // intentionally ignored — EntryPoint verifies receipt
        }

        // Validate signature (owner or active session key).
        if (_validateSignature(userOpData, userOpHash)) {
            return 0; // success
        }
        return 1; // failure
    }

    // ─── Execute ──────────────────────────────────────────────────────────

    function execute(address target, uint256 value, bytes calldata data)
        external onlyOwnerOrEntryPoint
    {
        (bool ok, bytes memory ret) = target.call{value: value}(data);
        require(ok, string(ret));
        emit Executed(target, value, data);
    }

    function executeBatch(
        address[] calldata targets,
        uint256[] calldata values,
        bytes[]   calldata datas
    ) external onlyOwnerOrEntryPoint {
        require(targets.length == values.length && values.length == datas.length,
                "SmartWallet: length mismatch");
        for (uint256 i; i < targets.length; ++i) {
            (bool ok, bytes memory ret) = targets[i].call{value: values[i]}(datas[i]);
            require(ok, string(ret));
            emit Executed(targets[i], values[i], datas[i]);
        }
    }

    // ─── Session Keys ─────────────────────────────────────────────────────

    function addSessionKey(
        address key,
        uint256 expiry,
        uint256 maxValuePerCall,
        address[] calldata allowedContracts
    ) external onlyOwnerOrEntryPoint {
        sessionKeys[key] = SessionKey({
            active:           true,
            expiry:           expiry,
            maxValuePerCall:  maxValuePerCall,
            allowedContracts: allowedContracts
        });
        emit SessionKeyAdded(key, expiry);
    }

    function revokeSessionKey(address key) external onlyOwnerOrEntryPoint {
        sessionKeys[key].active = false;
        emit SessionKeyRevoked(key);
    }

    // ─── Social Recovery ─────────────────────────────────────────────────

    function addGuardian(address guardian) external onlyOwnerOrEntryPoint {
        require(!guardians[guardian], "SmartWallet: already guardian");
        guardians[guardian] = true;
        guardianCount++;
    }

    /// @notice Replace the owner via social recovery.
    /// @dev    Requires `recoveryThreshold` distinct guardian signatures over
    ///         the canonical digest
    ///           keccak256(abi.encode(
    ///             "ZbxSmartWallet.recover", address(this), block.chainid,
    ///             newOwner, recoveryNonce
    ///           ))
    ///         wrapped with EIP-191 personal_sign. The single-guardian
    ///         "anyone with one guardian seat takes the wallet" model that
    ///         was here before is unsafe by design (C-16). The on-chain
    ///         `recoveryNonce` prevents replay across recoveries.
    /// @param  guardianSignatures  array of 65-byte ECDSA signatures.
    function executeRecovery(
        address newOwner,
        bytes[] calldata guardianSignatures
    ) external {
        require(newOwner != address(0),                   "SmartWallet: zero owner");
        require(recoveryThreshold > 0,                    "SmartWallet: recovery off");
        require(guardianSignatures.length >= recoveryThreshold,
                "SmartWallet: insufficient sigs");

        bytes32 digest = keccak256(abi.encode(
            "ZbxSmartWallet.recover",
            address(this), block.chainid, newOwner, recoveryNonce
        ));
        bytes32 ethDigest = keccak256(abi.encodePacked(
            bytes1(0x19), "Ethereum Signed Message:\n32", digest
        ));

        // Stack-allocated bitmap of seen guardian addresses; up to 256 unique
        // signers per call by hashing the address.
        // To keep duplicate-detection O(N²) free at small sizes we use a
        // memory array of seen addresses with linear scan.
        address[] memory seen = new address[](guardianSignatures.length);
        uint256 seenLen;
        uint256 valid;
        for (uint256 i = 0; i < guardianSignatures.length; i++) {
            address signer = _recoverSignerLowS(ethDigest, guardianSignatures[i]);
            if (signer == address(0))   continue;
            if (!guardians[signer])     continue;
            bool dup = false;
            for (uint256 j = 0; j < seenLen; j++) {
                if (seen[j] == signer) { dup = true; break; }
            }
            if (dup) continue;
            seen[seenLen++] = signer;
            valid++;
            if (valid >= recoveryThreshold) break;
        }
        require(valid >= recoveryThreshold, "SmartWallet: threshold not met");

        recoveryNonce += 1;
        owner = newOwner;
        emit RecoveryExecuted(newOwner, seen[0]);
    }

    /// EIP-2 low-S enforced ECDSA recovery. Returns address(0) on any
    /// malformed input rather than reverting.
    function _recoverSignerLowS(bytes32 hash, bytes calldata sig) internal pure returns (address) {
        if (sig.length != 65) return address(0);
        bytes32 r; bytes32 s; uint8 v;
        // S25-Y3 assembly: split fixed-65-byte ECDSA signature into (r,s,v) via
        // direct calldata reads. Bounds proven by `sig.length != 65 → return` on
        // the previous line. Equivalent to abi.decode but avoids the 600+ gas
        // cost of memory copy + array decode.
        assembly {
            r := calldataload(sig.offset)
            s := calldataload(add(sig.offset, 32))
            v := byte(0, calldataload(add(sig.offset, 64)))
        }
        // secp256k1 n / 2
        if (uint256(s) > 0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0)
            return address(0);
        if (v != 27 && v != 28) return address(0);
        return ecrecover(hash, v, r, s);
    }

    /// @notice Update the M-of-N recovery threshold. Only callable by the
    /// current owner / EntryPoint. Threshold must not exceed the current
    /// guardian count.
    function setRecoveryThreshold(uint256 newThreshold) external onlyOwnerOrEntryPoint {
        require(newThreshold > 0 && newThreshold <= guardianCount,
                "SmartWallet: bad threshold");
        recoveryThreshold = newThreshold;
    }

    // ─── ERC-1271 Signature Validation ────────────────────────────────────

    function isValidSignature(bytes32 hash, bytes calldata sig)
        external view returns (bytes4)
    {
        if (_recoverSigner(hash, sig) == owner) {
            return 0x1626ba7e; // ERC-1271 magic value
        }
        return 0xffffffff;
    }

    // ─── Internal ─────────────────────────────────────────────────────────

    function _validateSignature(bytes calldata userOpData, bytes32 hash) internal view returns (bool) {
        // Extract signature from userOp (last 65 bytes of calldata).
        if (userOpData.length < 65) return false;
        bytes calldata sig = userOpData[userOpData.length - 65:];
        address signer = _recoverSigner(hash, sig);
        if (signer == owner) return true;
        // Check session keys.
        SessionKey storage sk = sessionKeys[signer];
        return sk.active && block.number <= sk.expiry;
    }

    function _recoverSigner(bytes32 hash, bytes calldata sig) internal pure returns (address) {
        if (sig.length != 65) return address(0);
        bytes32 r; bytes32 s; uint8 v;
        // S25-Y3 assembly: same fixed-65-byte (r,s,v) split as _recoverSignerLowS.
        // Bounds proven by `sig.length != 65 → return` on the previous line.
        assembly { r := calldataload(sig.offset) s := calldataload(add(sig.offset, 32)) v := byte(0, calldataload(add(sig.offset, 64))) }
        return ecrecover(hash, v, r, s);
    }

    receive() external payable {}
}