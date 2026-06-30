// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title ZbxEntryPoint — ERC-4337 Account Abstraction EntryPoint.
/// @notice Central contract for UserOperation processing on Zebvix Chain.
///         Smart wallets (accounts) are deployed per-user and interact
///         with DeFi protocols via the EntryPoint.
///
/// @dev   ERC-4337 flow:
///           1. User signs a UserOperation (not a raw tx).
///           2. Bundler submits UserOps via handleOps().
///           3. EntryPoint validates each op (signature + nonce + funds).
///           4. EntryPoint calls account.execute() for each op.
///           5. EntryPoint charges gas fee from account or paymaster.
///
///        Key benefits:
///           - Gasless transactions (paymaster pays)
///           - Batched operations (multiple calls in one op)
///           - Social recovery (no seed phrase lock-in)
///           - Session keys (temporary, limited-scope signers)
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:erc-4337   EntryPoint v0.6
///
/// @custom:audit-2026-04-30  S4-A1 (CRITICAL) closed:
///   - Pre-funds maxCost from balanceOf[payer] BEFORE execute().
///   - Real per-op cost = actual gas used * tx.gasprice; refunded.
///   - `success || true` tautology removed; the real success bool is
///     emitted to UserOperationEvent and surfaces as `false` for reverted ops.
///   - Paymaster lifecycle: validatePaymasterUserOp() pre-execute and
///     postOp() post-execute, with the canonical (mode, context, actualGasCost)
///     calldata. Paymaster failures revert the entire op (per ERC-4337).

import { ReentrancyGuard } from "./libraries/ReentrancyGuard.sol";

contract ZbxEntryPoint is ReentrancyGuard {

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

    // ─── Types ────────────────────────────────────────────────────────────

    struct UserOperation {
        address sender;                 // smart wallet address
        uint256 nonce;
        bytes   initCode;              // factory + init data (if deploying)
        bytes   callData;              // call to execute on sender
        uint256 callGasLimit;
        uint256 verificationGasLimit;
        uint256 preVerificationGas;
        uint256 maxFeePerGas;
        uint256 maxPriorityFeePerGas;
        bytes   paymasterAndData;     // optional paymaster + data
        bytes   signature;
    }

    /// Mode passed to paymaster.postOp().
    /// Must mirror the ERC-4337 v0.6 definition exactly so off-chain bundlers
    /// can decode it without ambiguity.
    uint8 internal constant POST_OP_SUCCESS         = 0;
    uint8 internal constant POST_OP_OP_REVERTED     = 1;

    // ─── State ────────────────────────────────────────────────────────────

    mapping(address => uint256) public balanceOf;     // deposited ZBX per account
    mapping(bytes32 => bool)    public usedNonces;

    uint256 public constant SIG_VALIDATION_FAILED  = 1;
    uint256 public constant SIG_VALIDATION_SUCCESS = 0;

    // ─── Reentrancy guard ────────────────────────────────────────────────
    // Audit-2026-05-01 S6-EP3: handleOps invokes `beneficiary.call` with
    // arbitrary value at the end of the loop. A malicious beneficiary can
    // re-enter handleOps before the outer call returns. Guard the public
    // entry point so concurrent batches cannot interleave nonce/billing
    // state mutations.
    // SEC-2026-05-09: migrated to libraries/ReentrancyGuard.sol.

    /// Sentinel "no deadline" returned by paymasters that don't care about
    /// validity windows. ERC-4337 v0.6 packs (validAfter, validUntil) into
    /// a single uint256; a zero validUntil conventionally means "no expiry".
    uint256 internal constant NO_DEADLINE = 0;

    // ─── Events ───────────────────────────────────────────────────────────

    event UserOperationEvent(
        bytes32 indexed userOpHash,
        address indexed sender,
        address indexed paymaster,
        uint256 nonce,
        bool    success,
        uint256 actualGasCost,
        uint256 actualGasUsed
    );
    event UserOperationRevertReason(
        bytes32 indexed userOpHash,
        address indexed sender,
        uint256 nonce,
        bytes   revertReason
    );
    event AccountDeployed(bytes32 indexed userOpHash, address indexed sender, address factory, address paymaster);
    event Deposited(address indexed account, uint256 totalDeposit);
    event Withdrawn(address indexed account, address withdrawAddress, uint256 amount);
    /// Audit-2026-05-01 S6-EP1: emitted when a paymaster's postOp() reverts.
    /// The op's gas was already settled; the paymaster eats its own loss.
    /// Off-chain bundlers should blacklist abusive paymasters that emit this.
    event PostOpReverted(bytes32 indexed userOpHash, address indexed paymaster, uint256 actualGasCost);

    // ─── Core: handleOps ─────────────────────────────────────────────────

    /// @notice Process a batch of UserOperations.
    /// @param ops          List of UserOperations to process.
    /// @param beneficiary  Address receiving the collected gas fees.
    function handleOps(UserOperation[] calldata ops, address payable beneficiary)
        external
        nonReentrant
    {
        require(beneficiary != address(0), "EntryPoint: beneficiary=0");
        uint256 totalGasCollected;
        for (uint256 i; i < ops.length; ++i) {
            totalGasCollected += _handleOp(ops[i]);
        }
        if (totalGasCollected > 0) {
            (bool sent, ) = beneficiary.call{value: totalGasCollected}("");
            require(sent, "EntryPoint: beneficiary transfer failed");
        }
    }

    function _handleOp(UserOperation calldata op) internal returns (uint256 gasCollected) {
        bytes32 opHash = getUserOpHash(op);

        // 1. Deploy account if initCode provided.
        if (op.initCode.length > 0 && op.sender.code.length == 0) {
            _deployAccount(op, opHash);
        }

        // 2. Validate UserOperation (nonce + signature). Computes maxCost +
        //    resolves the payer (paymaster ?? sender). Pre-charges the payer
        //    so we never execute a UserOp we cannot bill.
        (uint256 maxCost, address payer, address paymaster, bytes memory pmContext)
            = _validateAndPrefund(op, opHash);

        // 3. Execute the operation. We track the **actual** success of the
        //    inner call so the emitted event is truthful and any paymaster
        //    postOp sees the real outcome. Reverts inside `op.callData` do
        //    NOT bubble up — that would crash the bundler's whole batch —
        //    but they are surfaced via UserOperationRevertReason and the
        //    `success` field of UserOperationEvent.
        uint256 gasBefore = gasleft();
        (bool success, bytes memory ret) = op.sender.call{gas: op.callGasLimit}(op.callData);
        uint256 gasUsed = gasBefore - gasleft() + op.preVerificationGas + op.verificationGasLimit;

        // 4. Compute the real billed amount, capped at the pre-funded maxCost.
        uint256 actualGasCost = gasUsed * tx.gasprice;
        if (actualGasCost > maxCost) {
            actualGasCost = maxCost;
        }

        // 5. Refund the unused portion of the prefund back to the payer.
        unchecked {
            balanceOf[payer] += (maxCost - actualGasCost);
        }
        gasCollected = actualGasCost;

        // 6. Paymaster postOp lifecycle. ERC-4337 requires this happens
        //    AFTER execution, with the real (mode, context, actualGasCost).
        //
        //    Audit-2026-05-01 S6-EP1 (CRITICAL): a previous version did
        //    `require(pmOk)` and bubbled paymaster reverts up — meaning a
        //    malicious paymaster could revert in postOp() to crash the
        //    bundler's entire batch (DoS for every co-bundled UserOp).
        //
        //    Per ERC-4337 v0.6 the paymaster has already been pre-charged
        //    the maxCost in `_validateAndPrefund`; the gas billing has
        //    already been settled at lines 128–137 above. A reverting
        //    postOp therefore only loses the paymaster its own opportunity
        //    to do bookkeeping — the protocol must NOT lose the whole
        //    batch on its behalf. We surface the failure via an event so
        //    bundlers can blacklist abusive paymasters off-chain.
        //
        //    Audit-2026-05-01 S6-EP1 (residual hardening, post-architect-review):
        //    bound the postOp call to `op.verificationGasLimit` so a malicious
        //    paymaster cannot burn the bundler's full remaining gas budget
        //    inside postOp. The `if (!pmOk)` branch absorbs OOG (low-level
        //    `.call` returns false on out-of-gas), so the batch now survives
        //    even an adversarial gas-grief paymaster. Bundlers should reject
        //    UserOps whose verificationGasLimit is unreasonably high at
        //    simulation time as standard hygiene.
        if (paymaster != address(0)) {
            uint8 mode = success ? POST_OP_SUCCESS : POST_OP_OP_REVERTED;
            (bool pmOk, ) = paymaster.call{gas: op.verificationGasLimit}(
                abi.encodeWithSignature(
                    "postOp(uint8,bytes,uint256)",
                    mode, pmContext, actualGasCost
                )
            );
            if (!pmOk) {
                emit PostOpReverted(opHash, paymaster, actualGasCost);
            }
        }

        // 7. Surface the real outcome via events (NEVER hardcoded to true).
        if (!success) {
            emit UserOperationRevertReason(opHash, op.sender, op.nonce, ret);
        }
        emit UserOperationEvent(
            opHash, op.sender, paymaster,
            op.nonce, success, actualGasCost, gasUsed
        );
    }

    /// Validate the userOp and pre-charge `maxCost` to the resolved payer
    /// (paymaster if `paymasterAndData.length >= 20`, else the sender).
    function _validateAndPrefund(
        UserOperation calldata op,
        bytes32 opHash
    )
        internal
        returns (
            uint256 maxCost,
            address payer,
            address paymaster,
            bytes memory pmContext
        )
    {
        // 1. Replay-resistant nonce check.
        bytes32 nonceKey = keccak256(abi.encodePacked(op.sender, op.nonce));
        require(!usedNonces[nonceKey], "EntryPoint: nonce already used");
        usedNonces[nonceKey] = true;

        // 2. Bound the gas budget that will be charged.
        maxCost = (op.callGasLimit + op.verificationGasLimit + op.preVerificationGas)
            * op.maxFeePerGas;

        paymaster = _paymaster(op);
        payer = paymaster == address(0) ? op.sender : paymaster;

        // 3. Sender-side signature validation (delegated to the smart wallet).
        (bool ok, bytes memory ret) = op.sender.call(
            abi.encodeWithSignature(
                "validateUserOp((address,uint256,bytes,bytes,uint256,uint256,uint256,uint256,uint256,bytes,bytes),bytes32,uint256)",
                op, opHash, maxCost
            )
        );
        require(ok && ret.length >= 32, "EntryPoint: account validation reverted");
        uint256 sigData = abi.decode(ret, (uint256));
        require(sigData == SIG_VALIDATION_SUCCESS, "EntryPoint: signature invalid");

        // 4. Paymaster-side validation. The paymaster must explicitly accept
        //    this op (and may reject blocked users — see ZbxPaymaster.sol).
        //
        //    Audit-2026-05-01 S6-EP2 (HIGH): the second tuple element
        //    returned from validatePaymasterUserOp is the paymaster's
        //    `validUntil` deadline (a unix timestamp; 0 = no deadline).
        //    Previously discarded — meaning ops could be bundled and
        //    executed long after the paymaster intended their validity to
        //    expire. We now decode and enforce it.
        if (paymaster != address(0)) {
            bytes calldata pmData = op.paymasterAndData[20:];
            (bool pmOk, bytes memory pmRet) = paymaster.call(
                abi.encodeWithSignature(
                    "validatePaymasterUserOp(address,bytes32,uint256,bytes)",
                    op.sender, opHash, maxCost, pmData
                )
            );
            require(pmOk, "EntryPoint: paymaster validation reverted");
            uint256 validUntil;
            (pmContext, validUntil) = abi.decode(pmRet, (bytes, uint256));
            require(
                validUntil == NO_DEADLINE || block.timestamp <= validUntil,
                "EntryPoint: paymaster validity expired"
            );
        }

        // 5. Pre-fund: deduct the maximum possible cost up front. Refund of
        //    the unused portion happens after execute() in _handleOp.
        require(balanceOf[payer] >= maxCost, "EntryPoint: insufficient deposit");
        unchecked {
            balanceOf[payer] -= maxCost;
        }
    }

    function _deployAccount(UserOperation calldata op, bytes32 opHash) internal {
        address factory = address(bytes20(op.initCode[:20]));
        bytes calldata initCallData = op.initCode[20:];
        (bool success, ) = factory.call(initCallData);
        require(success, "EntryPoint: factory deployment failed");
        require(op.sender.code.length > 0, "EntryPoint: account not deployed");
        emit AccountDeployed(opHash, op.sender, factory, _paymaster(op));
    }

    // ─── Deposit / Withdraw ───────────────────────────────────────────────

    function depositTo(address account) external payable {
        balanceOf[account] += msg.value;
        emit Deposited(account, balanceOf[account]);
    }

    function withdrawTo(address payable to, uint256 amount) external {
        require(balanceOf[msg.sender] >= amount, "EntryPoint: insufficient deposit");
        balanceOf[msg.sender] -= amount;
        (bool sent, ) = to.call{value: amount}("");
        require(sent, "EntryPoint: withdraw transfer failed");
        emit Withdrawn(msg.sender, to, amount);
    }

    // ─── Views ────────────────────────────────────────────────────────────

    function getUserOpHash(UserOperation calldata op) public view returns (bytes32) {
        return keccak256(abi.encode(
            keccak256(abi.encode(
                op.sender, op.nonce, keccak256(op.initCode), keccak256(op.callData),
                op.callGasLimit, op.verificationGasLimit, op.preVerificationGas,
                op.maxFeePerGas, op.maxPriorityFeePerGas, keccak256(op.paymasterAndData)
            )),
            address(this),
            block.chainid
        ));
    }

    function _paymaster(UserOperation calldata op) internal pure returns (address) {
        return op.paymasterAndData.length >= 20
            ? address(bytes20(op.paymasterAndData[:20]))
            : address(0);
    }

    receive() external payable { balanceOf[msg.sender] += msg.value; }
}
