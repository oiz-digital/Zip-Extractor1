/**
 * AaHelper — Account Abstraction (ZEP-002, ZVM AASENDER opcode).
 * Builds and submits EIP-4337-style UserOperations to the AA bundler.
 *
 * Enables:
 * - Gasless transactions (sponsored by a paymaster)
 * - Batch operations in a single UserOp
 * - Social recovery wallets
 * - Pay ID as smart wallet address
 *
 * @example
 * import { AaHelper } from "zebvix.js";
 *
 * const aa = new AaHelper(client);
 *
 * // Build a UserOp to send ZBX gaslessly
 * const op = aa.buildUserOp({
 *   sender: "0xSmartWallet...",
 *   to:     "ali@zbx",
 *   value:  "100",
 * });
 *
 * // Sign and submit to bundler
 * const hash = await aa.submit(op, signerWallet);
 * const receipt = await aa.waitForUserOp(hash);
 */
import type { ZbxClient } from "./client";
import { keccak_256 } from "@noble/hashes/sha3";

export interface UserOperation {
  sender:               string;
  nonce:                bigint;
  initCode:             string;
  callData:             string;
  callGasLimit:         bigint;
  verificationGasLimit: bigint;
  preVerificationGas:   bigint;
  maxFeePerGas:         bigint;
  maxPriorityFeePerGas: bigint;
  paymasterAndData:     string;
  signature:            string;
}

export interface UserOpReceipt {
  userOpHash: string;
  txHash:     string;
  success:    boolean;
  actualGasUsed: bigint;
}

export class AaHelper {
  private readonly ENTRYPOINT = "0x5FF137D4b0FDCD49DcA30c7CF57E578a026d2789";
  private readonly BUNDLER_RPC: string;

  constructor(
    private readonly client: ZbxClient,
    bundlerUrl?: string,
  ) {
    this.BUNDLER_RPC = bundlerUrl ?? (client as any).rpcUrl;
  }

  /**
   * Build a UserOperation for a simple ZBX transfer.
   *
   * @example
   * const op = await aa.buildTransfer({
   *   sender: "0xSmartWallet...",
   *   to:     "ali@zbx",       // Pay ID auto-resolved
   *   value:  "100",
   * });
   */
  async buildTransfer(params: {
    sender:  string;
    to:      string;
    value:   string;
    paymaster?: string;
  }): Promise<UserOperation> {
    // Resolve Pay ID if needed
    let toAddress = params.to;
    if (params.to.endsWith("@zbx")) {
      const resolved = await this.client.rpc<string>("zbx_resolvePayId", [params.to]);
      if (!resolved || resolved === "0x0000000000000000000000000000000000000000") {
        throw new Error(`Pay ID not found: \${params.to}`);
      }
      toAddress = resolved;
    }

    const valueWei = parseWei(params.value);
    const nonce    = await this.getAccountNonce(params.sender);

    // Encode execute(address to, uint256 value, bytes calldata data)
    const callData = "0xb61d27f6" // execute selector
      + toAddress.slice(2).padStart(64, "0")
      + valueWei.toString(16).padStart(64, "0")
      + "0".repeat(64); // empty data offset

    return {
      sender:               params.sender,
      nonce,
      initCode:             "0x",
      callData,
      callGasLimit:         50000n,
      verificationGasLimit: 100000n,
      preVerificationGas:   21000n,
      maxFeePerGas:         await this.getGasPrice(),
      maxPriorityFeePerGas: 1000000000n, // 1 gwei tip
      paymasterAndData:     params.paymaster ?? "0x",
      signature:            "0x" + "0".repeat(130), // placeholder, must be filled by sign()
    };
  }

  /**
   * Build a batch UserOperation (multiple calls in one tx).
   *
   * @example
   * const op = await aa.buildBatch({
   *   sender: "0xSmartWallet...",
   *   calls: [
   *     { to: "ali@zbx",  value: "10" },
   *     { to: "bob@zbx",  value: "20" },
   *     { to: "0xContract...", data: "0xabcd..." },
   *   ],
   * });
   */
  async buildBatch(params: {
    sender: string;
    calls:  Array<{ to: string; value?: string; data?: string }>;
    paymaster?: string;
  }): Promise<UserOperation> {
    // Resolve all Pay IDs
    const resolvedCalls = await Promise.all(params.calls.map(async call => {
      let to = call.to;
      if (to.endsWith("@zbx")) {
        const resolved = await this.client.rpc<string>("zbx_resolvePayId", [to]);
        if (resolved && resolved !== "0x0000000000000000000000000000000000000000") to = resolved;
      }
      return { to, value: call.value ?? "0", data: call.data ?? "0x" };
    }));

    const nonce = await this.getAccountNonce(params.sender);

    // Encode executeBatch(address[], uint256[], bytes[])
    const callData = "0x18dfb3c7" // executeBatch selector
      + encodeBatch(resolvedCalls);

    const totalGas = BigInt(resolvedCalls.length) * 30000n + 50000n;

    return {
      sender:               params.sender,
      nonce,
      initCode:             "0x",
      callData,
      callGasLimit:         totalGas,
      verificationGasLimit: 100000n,
      preVerificationGas:   21000n,
      maxFeePerGas:         await this.getGasPrice(),
      maxPriorityFeePerGas: 1000000000n,
      paymasterAndData:     params.paymaster ?? "0x",
      signature:            "0x" + "0".repeat(130),
    };
  }

  /**
   * Submit a signed UserOperation to the bundler.
   * Returns the userOpHash.
   *
   * @example
   * const op   = await aa.buildTransfer({ sender, to: "ali@zbx", value: "100" });
   * const hash = await aa.submit(op);
   * const rcpt = await aa.waitForUserOp(hash);
   */
  async submit(op: UserOperation): Promise<string> {
    return this.bundlerRpc("eth_sendUserOperation", [serializeUserOp(op), this.ENTRYPOINT]);
  }

  /**
   * Wait for a UserOperation to be included in a block.
   *
   * @example
   * const receipt = await aa.waitForUserOp(hash);
   * console.log("Success:", receipt.success);
   * console.log("Tx hash:", receipt.txHash);
   */
  async waitForUserOp(userOpHash: string, timeoutMs = 60_000): Promise<UserOpReceipt> {
    const start = Date.now();
    while (Date.now() - start < timeoutMs) {
      const receipt = await this.bundlerRpc<UserOpReceipt | null>(
        "eth_getUserOperationReceipt",
        [userOpHash]
      );
      if (receipt) return receipt;
      await delay(2000);
    }
    throw new Error(`UserOp \${userOpHash} not confirmed within \${timeoutMs}ms`);
  }

  /**
   * SEC-2026-05-09 (S3): EIP-4337-compliant UserOp hash.
   *
   * Previously this used a 32-bit DJB string hash (`hash = hash * 33 ^ ch`)
   * which is **not** a cryptographic hash and is trivially collidable —
   * any signature produced over that hash would be meaningless. The new
   * implementation matches the canonical 4337 spec:
   *
   *   userOpHash = keccak256(abi.encode(
   *       keccak256(packedUserOp),
   *       entrypoint,
   *       chainId,
   *   ))
   *
   * where `packedUserOp` substitutes `keccak256(initCode)`,
   * `keccak256(callData)`, and `keccak256(paymasterAndData)` for the raw
   * dynamic fields per the spec.
   */
  async hashUserOp(op: UserOperation, chainId?: number): Promise<string> {
    const cid = chainId ?? await this.resolveChainId();
    const initCodeHash         = keccak256Bytes(hexToBytes(op.initCode));
    const callDataHash         = keccak256Bytes(hexToBytes(op.callData));
    const paymasterAndDataHash = keccak256Bytes(hexToBytes(op.paymasterAndData));

    // ABI-encode the packed userOp (all fields padded to 32 bytes).
    const packed = concatBytes(
      addrTo32(op.sender),
      uint256To32(op.nonce),
      initCodeHash,
      callDataHash,
      uint256To32(op.callGasLimit),
      uint256To32(op.verificationGasLimit),
      uint256To32(op.preVerificationGas),
      uint256To32(op.maxFeePerGas),
      uint256To32(op.maxPriorityFeePerGas),
      paymasterAndDataHash,
    );
    const packedHash = keccak256Bytes(packed);

    // Wrap with (entrypoint, chainId) per EIP-4337.
    const outer = concatBytes(
      packedHash,
      addrTo32(this.ENTRYPOINT),
      uint256To32(BigInt(cid)),
    );
    const finalHash = keccak256Bytes(outer);
    return "0x" + bytesToHex(finalHash);
  }

  private cachedChainId: number | null = null;
  private async resolveChainId(): Promise<number> {
    if (this.cachedChainId !== null) return this.cachedChainId;
    const hex = await this.client.rpc<string>("eth_chainId");
    const id = Number.parseInt(hex.replace(/^0x/, ""), 16);
    if (!Number.isFinite(id) || id <= 0) {
      throw new Error(`S3: eth_chainId returned invalid value: ${hex}`);
    }
    this.cachedChainId = id;
    return id;
  }

  /** Get account nonce from EntryPoint */
  private async getAccountNonce(sender: string): Promise<bigint> {
    const n = await this.client.rpc<string>("eth_call", [{
      to:   this.ENTRYPOINT,
      data: "0x35567e1a" + sender.slice(2).padStart(64, "0") + "0".repeat(64),
    }, "latest"]);
    return BigInt(n || "0");
  }

  private async getGasPrice(): Promise<bigint> {
    const hex = await this.client.rpc<string>("eth_gasPrice");
    return BigInt(hex);
  }

  private async bundlerRpc<T = unknown>(method: string, params: unknown[]): Promise<T> {
    const res = await fetch(this.BUNDLER_RPC, {
      method:  "POST",
      headers: { "Content-Type": "application/json" },
      body:    JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
    });
    const json = await res.json() as { result?: T; error?: { message: string } };
    if (json.error) throw new Error(`Bundler RPC: \${json.error.message}`);
    return json.result as T;
  }
}

function parseWei(amount: string): bigint {
  const [whole, frac = ""] = amount.split(".");
  return BigInt(whole) * 10n ** 18n + BigInt(frac.padEnd(18, "0").slice(0, 18) || "0");
}

function encodeBatch(calls: Array<{ to: string; value: string; data: string }>): string {
  return calls.map(c =>
    c.to.slice(2).padStart(64, "0")
    + parseWei(c.value).toString(16).padStart(64, "0")
    + "0".repeat(64)
  ).join("");
}

function serializeUserOp(op: UserOperation): Record<string, string> {
  const h = (n: bigint) => "0x" + n.toString(16);
  return {
    sender:               op.sender,
    nonce:                h(op.nonce),
    initCode:             op.initCode,
    callData:             op.callData,
    callGasLimit:         h(op.callGasLimit),
    verificationGasLimit: h(op.verificationGasLimit),
    preVerificationGas:   h(op.preVerificationGas),
    maxFeePerGas:         h(op.maxFeePerGas),
    maxPriorityFeePerGas: h(op.maxPriorityFeePerGas),
    paymasterAndData:     op.paymasterAndData,
    signature:            op.signature,
  };
}

function delay(ms: number): Promise<void> {
  return new Promise(r => setTimeout(r, ms));
}

// ── SEC-2026-05-09 (S3): byte / hash helpers for EIP-4337 userOp hash ────────

function keccak256Bytes(b: Uint8Array): Uint8Array {
  return keccak_256(b);
}

function hexToBytes(hex: string): Uint8Array {
  const clean = hex.replace(/^0x/, "");
  if (clean.length === 0) return new Uint8Array(0);
  const bytes = new Uint8Array(clean.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  }
  return bytes;
}

function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes, b => b.toString(16).padStart(2, "0")).join("");
}

function concatBytes(...arrs: Uint8Array[]): Uint8Array {
  const total = arrs.reduce((n, a) => n + a.length, 0);
  const out = new Uint8Array(total);
  let off = 0;
  for (const a of arrs) { out.set(a, off); off += a.length; }
  return out;
}

function uint256To32(n: bigint): Uint8Array {
  if (n < 0n) throw new Error("S3: uint256 cannot be negative");
  let hex = n.toString(16);
  if (hex.length > 64) throw new Error("S3: value exceeds uint256");
  hex = hex.padStart(64, "0");
  return hexToBytes(hex);
}

function addrTo32(addr: string): Uint8Array {
  const clean = addr.replace(/^0x/, "").toLowerCase();
  if (clean.length !== 40) throw new Error(`S3: bad address length: ${addr}`);
  return hexToBytes("0".repeat(24) + clean);
}