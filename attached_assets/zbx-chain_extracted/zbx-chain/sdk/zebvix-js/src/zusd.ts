/**
 * ZusdHelper — ZUSD stablecoin queries.
 * Accessible via `client.zusd.*`
 */
import type { ZbxClient } from "./client";

export class ZusdHelper {
  constructor(private client: ZbxClient) {}

  /**
   * Get ZUSD balance of an address.
   *
   * @example
   * const bal = await zbx.zusd.balanceOf("0x742d35...");
   * console.log(zbx.zusd.format(bal)); // "1250.50"
   */
  async balanceOf(address: string): Promise<bigint> {
    const raw = await this.client.rpc<string>("zbx_getZusdBalance", [address]);
    return BigInt(raw || "0");
  }

  /**
   * Format ZUSD wei to a human-readable string.
   * @example
   * zbx.zusd.format(1_250_500_000_000_000_000_000n); // "1250.5"
   */
  format(wei: bigint, decimals = 2): string {
    const whole = wei / 10n ** 18n;
    const frac  = wei % 10n ** 18n;
    if (frac === 0n) return whole.toString();
    const fracStr = frac.toString().padStart(18, "0").slice(0, decimals);
    return `\${whole}.\${fracStr.replace(/0+$/, "") || "0"}`;
  }

  /** Parse ZUSD decimal string to wei. */
  parse(amount: string): bigint {
    const [whole, frac = ""] = amount.split(".");
    const fracPadded = frac.padEnd(18, "0").slice(0, 18);
    return BigInt(whole) * 10n ** 18n + BigInt(fracPadded || "0");
  }

  /**
   * Get current ZUSD/ZBX exchange rate.
   * Returns how many ZUSD you get per 1 ZBX at the current pool price.
   */
  async zbxToUsd(): Promise<string> {
    const pool = await this.client.getPool();
    return pool.spotPriceUsdPerZbx ?? "0";
  }
}