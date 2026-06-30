/**
 * ZUSD stablecoin helper for ZBX chain.
 *
 * @example
 * import { ZUSD } from "@zebvix/ethers";
 *
 * const bal = await ZUSD.balanceOf("0x742d...", provider);
 * console.log(`\${ZUSD.format(bal)} ZUSD`);
 */
import { ZbxProvider } from "./provider";

export const ZUSD = {

  /** ZUSD token symbol */
  symbol: "ZUSD" as const,

  /** ZUSD decimals (18, same as ZBX) */
  decimals: 18 as const,

  /** ZUSD is pegged 1:1 to USD */
  pegCurrency: "USD" as const,

  /**
   * Get ZUSD balance of an address.
   *
   * @example
   * const bal = await ZUSD.balanceOf("0x742d35...", provider);
   * console.log(ZUSD.format(bal)); // "1250.5"
   */
  async balanceOf(address: string, provider: ZbxProvider): Promise<bigint> {
    return provider.zbx.zusdBalance(address);
  },

  /**
   * Format ZUSD wei to a human-readable string.
   *
   * @example
   * ZUSD.format(1_250_500_000_000_000_000_000n); // "1250.5"
   * ZUSD.format(0n); // "0"
   */
  format(wei: bigint, maxDecimals = 2): string {
    const whole = wei / 10n ** 18n;
    const frac  = wei % 10n ** 18n;
    if (frac === 0n) return whole.toString();
    const fracStr = frac.toString().padStart(18, "0").slice(0, maxDecimals);
    const trimmed = fracStr.replace(/0+$/, "");
    return trimmed ? `\${whole}.\${trimmed}` : whole.toString();
  },

  /**
   * Parse a ZUSD amount string to wei.
   *
   * @example
   * ZUSD.parse("1250.50"); // 1_250_500_000_000_000_000_000n
   */
  parse(amount: string): bigint {
    const [whole, frac = ""] = amount.split(".");
    const fracPadded = frac.padEnd(18, "0").slice(0, 18);
    return BigInt(whole) * 10n ** 18n + BigInt(fracPadded);
  },

  /**
   * Get current ZUSD/ZBX exchange rate (from pool oracle).
   * Returns how many ZUSD you get per 1 ZBX.
   *
   * @example
   * const rate = await ZUSD.zbxToUsd(provider);
   * console.log(`1 ZBX = \${rate} ZUSD`);
   */
  async zbxToUsd(provider: ZbxProvider): Promise<string> {
    const priceInfo = await provider.zbx.price();
    return priceInfo.zbxUsd;
  },
};