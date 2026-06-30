/**
 * ZVM client utilities for ZBX chain.
 * Lets you interact with ZVM-native contracts and decode ZVM-specific data.
 *
 * @example
 * import { ZvmClient } from "@zebvix/ethers";
 *
 * const zvm = new ZvmClient(provider);
 * const price = await zvm.zbxPrice();
 * const time  = await zvm.blockTime();
 */
import { ZbxProvider } from "./provider";
import type { ZvmResult } from "./types";

export class ZvmClient {
  constructor(private provider: ZbxProvider) {}

  /**
   * Get current ZBX/USD price via ZVM oracle (ZBXPRICE opcode).
   * This reads the same value as the ZVM 0xC2 ZBXPRICE opcode.
   *
   * @example
   * const price = await zvm.zbxPrice(); // "2500.00"
   */
  async zbxPrice(): Promise<string> {
    const info = await this.provider.zbx.price();
    return info.zbxUsd;
  }

  /**
   * Get ZBX block time in milliseconds (always 5000 for ZBX mainnet).
   * Same as ZVM ZBXTIME opcode (0xC3).
   */
  blockTime(): number {
    return 5000;
  }

  /**
   * Get ZVM version (same as CHAINVER opcode 0xC5).
   */
  zvmVersion(): number {
    return 1;
  }

  /**
   * Simulate a contract call using the ZVM (read-only, no state change).
   * Uses eth_call under the hood — ZVM executes the call on-chain.
   *
   * @example
   * const result = await zvm.call({
   *   to: "0x742d35...",
   *   data: "0xc200",  // ZBXPRICE opcode
   * });
   */
  async call(params: {
    to: string;
    data?: string;
    from?: string;
    value?: bigint;
  }): Promise<ZvmResult> {
    const result = await this.provider.call({
      to:    params.to,
      data:  params.data ?? "0x",
      from:  params.from,
      value: params.value,
    });
    return {
      status:     "success",
      returnData: result,
      gasUsed:    0n, // eth_call doesn't return gas used
    };
  }

  /**
   * Decode ZUSD balance from ZUSDBAL opcode result.
   * ZUSDBAL (0xC1) pushes a uint256 onto the stack.
   *
   * @example
   * const raw = "0x0000000000000000000000000000000000000000002b992a9c5ce4258d400000";
   * const bal = ZvmClient.decodeZusdBal(raw); // 207380000000000000000000n
   */
  static decodeZusdBal(hex: string): bigint {
    return BigInt(hex);
  }

  /**
   * Decode ZBXPRICE opcode result (uint256, 18 decimals).
   *
   * @example
   * const raw = "0x000000000000000000000000000000000000000000021e19e0c9bab2400000";
   * const usd = ZvmClient.decodeZbxPrice(raw);
   * // "2500" (meaning $2500 per ZBX)
   */
  static decodeZbxPrice(hex: string): string {
    const wei = BigInt(hex);
    const usd = wei / 10n ** 18n;
    return usd.toString();
  }

  /**
   * Check if bytecode is a ZVM-native contract (has magic prefix 0xEF5A42).
   *
   * @example
   * const code = await provider.getCode("0x742d35...");
   * ZvmClient.isZvmNative(code); // true/false
   */
  static isZvmNative(bytecode: string): boolean {
    const hex = bytecode.startsWith("0x") ? bytecode.slice(2) : bytecode;
    return hex.startsWith("ef5a42");
  }

  /**
   * Decode ZVM structured log entries from transaction receipt.
   *
   * ZVM ZVMLOG opcode (0xC9) emits key-value logs stored in the receipt.
   * This helper parses them into a readable format.
   */
  static decodeZvmLogs(logs: Array<{ topics: string[]; data: string }>): Array<{ key: string; value: string }> {
    return logs
      .filter(log => log.topics[0] === "0x5a564d4c4f470000000000000000000000000000000000000000000000000000") // ZVMLOG selector
      .map(log => {
        try {
          const data = Buffer.from(log.data.slice(2), "hex").toString("utf8");
          const [key, value] = data.split("=");
          return { key: key?.trim() ?? "", value: value?.trim() ?? "" };
        } catch {
          return { key: "", value: log.data };
        }
      });
  }
}