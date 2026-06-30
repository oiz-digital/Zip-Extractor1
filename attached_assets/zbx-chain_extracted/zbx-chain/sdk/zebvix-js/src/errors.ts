/**
 * Typed error classes for zebvix.js.
 * All ZBX errors extend ZbxError.
 *
 * @example
 * try {
 *   await wallet.send("ali@zbx", "100");
 * } catch (err) {
 *   if (err instanceof ZbxPayIdNotFoundError) {
 *     console.log("Pay ID not found:", err.payId);
 *   } else if (err instanceof ZbxInsufficientBalanceError) {
 *     console.log("Need:", err.required, "Have:", err.actual);
 *   }
 * }
 */

/** Base error for all ZBX SDK errors */
export class ZbxError extends Error {
  constructor(message: string, public readonly code: string) {
    super(message);
    this.name = "ZbxError";
  }
}

/** Pay ID not found in registry */
export class ZbxPayIdNotFoundError extends ZbxError {
  constructor(public readonly payId: string) {
    super(`Pay ID not found: '\${payId}'`, "PAY_ID_NOT_FOUND");
    this.name = "ZbxPayIdNotFoundError";
  }
}

/** Pay ID format is invalid */
export class ZbxPayIdInvalidError extends ZbxError {
  constructor(public readonly payId: string, reason: string) {
    super(`Invalid Pay ID '\${payId}': \${reason}`, "PAY_ID_INVALID");
    this.name = "ZbxPayIdInvalidError";
  }
}

/** Pay ID already registered */
export class ZbxPayIdTakenError extends ZbxError {
  constructor(public readonly payId: string, public readonly owner: string) {
    super(`Pay ID '\${payId}' is already registered to \${owner}`, "PAY_ID_TAKEN");
    this.name = "ZbxPayIdTakenError";
  }
}

/** Insufficient ZBX balance */
export class ZbxInsufficientBalanceError extends ZbxError {
  constructor(
    public readonly required: string,
    public readonly actual:   string,
    public readonly token = "ZBX",
  ) {
    super(`Insufficient \${token}: need \${required}, have \${actual}`, "INSUFFICIENT_BALANCE");
    this.name = "ZbxInsufficientBalanceError";
  }
}

/** Transaction reverted on-chain */
export class ZbxRevertError extends ZbxError {
  constructor(
    public readonly txHash:  string,
    public readonly reason?: string,
  ) {
    super(`Transaction reverted\${reason ? ": " + reason : ""} (hash: \${txHash})`, "TX_REVERTED");
    this.name = "ZbxRevertError";
  }
}

/** RPC connection error */
export class ZbxRpcError extends ZbxError {
  constructor(
    message: string,
    public readonly statusCode?: number,
  ) {
    super(message, "RPC_ERROR");
    this.name = "ZbxRpcError";
  }
}

/** Transaction timed out waiting for confirmation */
export class ZbxTimeoutError extends ZbxError {
  constructor(public readonly txHash: string, public readonly timeoutMs: number) {
    super(`Tx \${txHash} not confirmed within \${timeoutMs}ms`, "TX_TIMEOUT");
    this.name = "ZbxTimeoutError";
  }
}

/** AMM swap would exceed slippage tolerance */
export class ZbxSlippageError extends ZbxError {
  constructor(
    public readonly priceImpact: string,
    public readonly tolerance:   string,
  ) {
    super(`Price impact \${priceImpact}% exceeds slippage tolerance \${tolerance}%`, "SLIPPAGE_EXCEEDED");
    this.name = "ZbxSlippageError";
  }
}

/** Contract call reverted */
export class ZbxContractRevertError extends ZbxError {
  constructor(
    public readonly contractAddress: string,
    public readonly reason?: string,
  ) {
    super(`Contract \${contractAddress} reverted\${reason ? ": " + reason : ""}`, "CONTRACT_REVERTED");
    this.name = "ZbxContractRevertError";
  }
}

/** UserOperation (AA) failed */
export class ZbxUserOpError extends ZbxError {
  constructor(
    public readonly userOpHash: string,
    reason?: string,
  ) {
    super(`UserOp \${userOpHash} failed\${reason ? ": " + reason : ""}`, "USER_OP_FAILED");
    this.name = "ZbxUserOpError";
  }
}

/** Position or CDP became liquidatable / was liquidated */
export class ZbxLiquidationError extends ZbxError {
  constructor(
    public readonly positionId: string,
    public readonly healthBps:  number,
    reason?: string,
  ) {
    super(
      `Position \${positionId} is liquidatable (health: \${healthBps} bps)\${reason ? ": " + reason : ""}`,
      "LIQUIDATABLE",
    );
    this.name = "ZbxLiquidationError";
  }
}

/** Bridge transaction failed or was rejected */
export class ZbxBridgeError extends ZbxError {
  constructor(
    public readonly nonce: string,
    reason?: string,
  ) {
    super(`Bridge tx \${nonce} failed\${reason ? ": " + reason : ""}`, "BRIDGE_FAILED");
    this.name = "ZbxBridgeError";
  }
}

/** Perpetuals position error (open/close/liquidate) */
export class ZbxPerpError extends ZbxError {
  constructor(
    public readonly marketId: number,
    reason?: string,
  ) {
    super(`Perp error on market \${marketId}\${reason ? ": " + reason : ""}`, "PERP_ERROR");
    this.name = "ZbxPerpError";
  }
}