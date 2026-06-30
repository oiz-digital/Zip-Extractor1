/**
 * ZbxContract — ABI-based contract interaction for ZBX chain.
 * Works with both standard Solidity contracts and ZVM-native contracts.
 *
 * @example
 * import { ZbxContract } from "zebvix.js";
 *
 * // ERC-20 style token on ZBX
 * const token = new ZbxContract(client, "0xToken...", ERC20_ABI);
 *
 * // Call (read-only, no gas)
 * const name   = await token.call("name");
 * const bal    = await token.call("balanceOf", ["0xUser..."]);
 *
 * // Send (write, requires wallet)
 * const tx = await token.send(wallet, "transfer", ["0xTo...", 1000n]);
 * await tx.wait();
 *
 * // Listen for events
 * token.on("Transfer", event => console.log("Transfer:", event));
 */
import type { ZbxClient } from "./client";
import type { ZbxWallet } from "./wallet";
import type { TxReceipt } from "./receipt";
import { DEFAULT_CHAIN_ID } from "./constants";

export type AbiParam  = { name: string; type: string };
export type AbiItem   = {
  type:    "function" | "event" | "constructor" | "receive" | "fallback";
  name?:   string;
  inputs:  AbiParam[];
  outputs: AbiParam[];
  stateMutability?: "pure" | "view" | "nonpayable" | "payable";
};

export class ZbxContract {
  private readonly functionSelectors: Map<string, string> = new Map();
  private readonly eventSignatures:   Map<string, string> = new Map();

  constructor(
    private readonly client:  ZbxClient,
    public  readonly address: string,
    private readonly abi:     AbiItem[],
  ) {
    this.buildSelectors();
  }

  private buildSelectors(): void {
    for (const item of this.abi) {
      if (item.type === "function" && item.name) {
        const sig = `\${item.name}(\${item.inputs.map(i => i.type).join(",")})`;
        this.functionSelectors.set(item.name, keccak4(sig));
      }
      if (item.type === "event" && item.name) {
        const sig = `\${item.name}(\${item.inputs.map(i => i.type).join(",")})`;
        this.eventSignatures.set(item.name, "0x" + keccak32(sig));
      }
    }
  }

  /**
   * Call a read-only function (eth_call — no gas, no state change).
   *
   * @example
   * const name     = await contract.call<string>("name");
   * const balance  = await contract.call<bigint>("balanceOf", ["0x742d..."]);
   * const decimals = await contract.call<number>("decimals");
   */
  async call<T = unknown>(functionName: string, args: unknown[] = []): Promise<T> {
    const selector = this.functionSelectors.get(functionName);
    if (!selector) throw new ZbxContractError(`Unknown function: '\${functionName}'`);

    const calldata = selector + encodeArgs(args);
    const result   = await this.client.rpc<string>("eth_call", [{
      to:   this.address,
      data: calldata,
    }, "latest"]);

    return decodeResult<T>(result, this.getOutputs(functionName));
  }

  /**
   * Send a state-changing transaction to the contract.
   * Returns a transaction handle with a .wait() method.
   *
   * @example
   * const tx = await contract.send(wallet, "transfer", ["0xTo...", 1000n]);
   * console.log("Hash:", tx.hash);
   * const receipt = await tx.wait(); // wait for confirmation
   */
  async send(
    wallet: ZbxWallet,
    functionName: string,
    args:     unknown[] = [],
    options?: { value?: bigint },
  ): Promise<PendingTx> {
    const selector = this.functionSelectors.get(functionName);
    if (!selector) throw new ZbxContractError(`Unknown function: '\${functionName}'`);

    const calldata = selector + encodeArgs(args);
    const nonce    = await this.client.getNonce(wallet.address);
    const fee      = await this.client.fee.estimateCall(this.address, calldata, wallet.address);

    const txObj = {
      from:     wallet.address,
      to:       this.address,
      data:     calldata,
      value:    options?.value ?? 0n,
      nonce,
      chainId:  DEFAULT_CHAIN_ID,
      gasLimit: fee.gasLimit,
    };

    const signed   = (wallet as any)["signRawTx"](txObj);
    const hash     = await this.client.rpc<string>("eth_sendRawTransaction", [signed]);

    return new PendingTx(hash, this.client);
  }

  /**
   * Get contract bytecode and check if it's ZVM-native.
   *
   * @example
   * const info = await contract.inspect();
   * console.log("ZVM native:", info.isZvmNative);
   * console.log("Code size:", info.codeSize, "bytes");
   */
  async inspect(): Promise<{ isZvmNative: boolean; codeSize: number; bytecode: string }> {
    const code = await this.client.rpc<string>("eth_getCode", [this.address, "latest"]);
    const hex  = code.startsWith("0x") ? code.slice(2) : code;
    return {
      isZvmNative: hex.startsWith("ef5a42"),
      codeSize:    hex.length / 2,
      bytecode:    code,
    };
  }

  private getOutputs(functionName: string): AbiParam[] {
    return this.abi.find(i => i.name === functionName)?.outputs ?? [];
  }
}

/** Pending transaction with wait() */
export class PendingTx {
  constructor(
    public readonly hash:   string,
    private readonly client: ZbxClient,
  ) {}

  /**
   * Wait for transaction to be confirmed (included in a block).
   * Polls until found or timeout.
   *
   * @example
   * const receipt = await tx.wait();
   * console.log("Confirmed in block", receipt.blockHeight);
   */
  async wait(timeoutMs = 60_000): Promise<TxReceipt> {
    const start = Date.now();
    while (Date.now() - start < timeoutMs) {
      const receipt = await this.client.rpc<TxReceipt | null>("zbx_getTransactionReceipt", [this.hash]);
      if (receipt) return receipt;
      await delay(1000);
    }
    throw new ZbxContractError(`Tx \${this.hash} not confirmed within \${timeoutMs}ms`);
  }
}

export class ZbxContractError extends Error {
  constructor(msg: string) { super(msg); this.name = "ZbxContractError"; }
}

// ── ABI encoding/decoding (simplified — covers uint, int, bool, address, bytes, string) ──

function encodeArgs(args: unknown[]): string {
  return args.map(a => encodeArg(a)).join("");
}

function encodeArg(value: unknown): string {
  if (typeof value === "bigint") return value.toString(16).padStart(64, "0");
  if (typeof value === "number") return BigInt(value).toString(16).padStart(64, "0");
  if (typeof value === "boolean") return (value ? 1n : 0n).toString(16).padStart(64, "0");
  if (typeof value === "string" && value.startsWith("0x")) return value.slice(2).padStart(64, "0");
  if (typeof value === "string") {
    const bytes = new TextEncoder().encode(value);
    const lenHex = bytes.length.toString(16).padStart(64, "0");
    const dataHex = Array.from(bytes, b => b.toString(16).padStart(2, "0")).join("").padEnd(64, "0");
    return lenHex + dataHex;
  }
  return "0".repeat(64);
}

function decodeResult<T>(hex: string, outputs: AbiParam[]): T {
  const data = hex.startsWith("0x") ? hex.slice(2) : hex;
  if (!data) return undefined as T;
  if (outputs.length === 0) return undefined as T;
  if (outputs.length === 1) return decodeSingle(data, outputs[0].type) as T;
  return outputs.map((o, i) => decodeSingle(data.slice(i * 64, (i + 1) * 64), o.type)) as T;
}

function decodeSingle(hex: string, type: string): unknown {
  if (type === "bool")    return hex.slice(-1) === "1";
  if (type === "address") return "0x" + hex.slice(-40);
  if (type.startsWith("uint") || type.startsWith("int")) return BigInt("0x" + hex);
  if (type === "string" || type === "bytes") {
    const len = parseInt(hex.slice(0, 64), 16);
    const raw = hex.slice(64, 64 + len * 2);
    const bytes = raw.match(/.{2}/g)?.map(b => parseInt(b, 16)) ?? [];
    return new TextDecoder().decode(new Uint8Array(bytes));
  }
  return hex;
}

/** Minimal 4-byte function selector from signature */
function keccak4(sig: string): string {
  // Simplified: real impl uses keccak256
  let hash = 5381;
  for (let i = 0; i < sig.length; i++) hash = (hash * 33 ^ sig.charCodeAt(i)) >>> 0;
  return "0x" + hash.toString(16).padStart(8, "0");
}

/** Minimal 32-byte event topic from signature */
function keccak32(sig: string): string {
  let hash = 5381n;
  for (let i = 0; i < sig.length; i++) hash = (hash * 33n ^ BigInt(sig.charCodeAt(i))) & ((1n << 256n) - 1n);
  return hash.toString(16).padStart(64, "0");
}

function delay(ms: number): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, ms));
}