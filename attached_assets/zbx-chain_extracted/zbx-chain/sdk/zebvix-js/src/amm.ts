/**
 * AmmHelper — ZBX/ZUSD AMM pool interaction.
 * The AMM follows Uniswap v2 style constant product formula: x * y = k
 *
 * @example
 * import { ZbxClient } from "zebvix.js";
 *
 * const zbx = new ZbxClient("http://127.0.0.1:8545");
 *
 * // Get pool state
 * const pool = await zbx.amm.pool();
 * console.log("ZBX reserve:", pool.zbxReserveZbx);
 * console.log("ZUSD reserve:", pool.zusdReserve);
 * console.log("Spot price: 1 ZBX =", pool.spotPriceUsd, "USD");
 *
 * // Quote: how much ZUSD for 10 ZBX in?
 * const quote = zbx.amm.quoteZbxIn("10", pool);
 * console.log("Get:", quote.amountOut, "ZUSD for 10 ZBX");
 *
 * // Add liquidity
 * const tx = await wallet.addLiquidity("1000", "2500000");
 * await tx.wait();
 *
 * // Remove liquidity
 * const tx = await wallet.removeLiquidity("500");
 * await tx.wait();
 */
import type { ZbxClient } from "./client";
import type { PoolInfo } from "./types";

export interface PoolState {
  initialized:     boolean;
  poolAddress:     string;
  zbxReserveWei:   bigint;
  zbxReserveZbx:   string;
  zusdReserveWei:  bigint;
  zusdReserve:     string;
  lpSupply:        bigint;
  lpSupplyStr:     string;
  spotPriceUsd:    string;  // USD per ZBX
  tvlUsd:          string;  // total value locked
}

export interface SwapQuote {
  amountIn:       string;
  amountOut:      string;
  priceImpact:    string;  // percentage
  spotBefore:     string;
  spotAfter:      string;
  fee:            string;
}

export interface LiquidityQuote {
  zbxIn:    string;
  zusdIn:   string;
  lpOut:    string;
  share:    string;  // percentage of pool
}

export class AmmHelper {
  private readonly POOL_ADDRESS = "0x0000000000000000000000005A42414d4d000000";
  private readonly FEE_BPS = 30n; // 0.30%

  constructor(private readonly client: ZbxClient) {}

  /**
   * Get current pool state.
   *
   * @example
   * const pool = await zbx.amm.pool();
   * console.log("Spot price: 1 ZBX =", pool.spotPriceUsd, "USD");
   */
  async pool(): Promise<PoolState> {
    const raw: PoolInfo = await this.client.rpc("zbx_getPool");
    const zbxWei  = BigInt(raw.zbxReserveWei  || "0");
    const zusdWei = BigInt(raw.zusdReserve    || "0");
    const lpWei   = BigInt(raw.lpSupply       || "0");

    const spot = zbxWei > 0n ? (zusdWei * 10n ** 18n / zbxWei) : 0n;
    const spotStr = (Number(spot) / 1e18).toFixed(2);
    const tvl = (Number(zusdWei) / 1e18 * 2).toFixed(0);

    return {
      initialized:    raw.initialized,
      poolAddress:    raw.poolAddress,
      zbxReserveWei:  zbxWei,
      zbxReserveZbx:  formatWei(zbxWei),
      zusdReserveWei: zusdWei,
      zusdReserve:    formatWei(zusdWei),
      lpSupply:       lpWei,
      lpSupplyStr:    formatWei(lpWei),
      spotPriceUsd:   spotStr,
      tvlUsd:         tvl,
    };
  }

  /**
   * Quote a ZBX-in swap (how much ZUSD you get for X ZBX).
   * Uses constant product formula with 0.30% fee.
   *
   * @example
   * const pool  = await zbx.amm.pool();
   * const quote = zbx.amm.quoteZbxIn("10", pool);
   * console.log(`10 ZBX → \${quote.amountOut} ZUSD (impact: \${quote.priceImpact}%)`);
   */
  quoteZbxIn(amountZbxStr: string, pool: PoolState): SwapQuote {
    const amountIn = parseWei(amountZbxStr);
    const x = pool.zbxReserveWei;
    const y = pool.zusdReserveWei;

    if (x === 0n || y === 0n) throw new Error("Pool not initialized");

    const amountInFee = amountIn * (10000n - this.FEE_BPS) / 10000n;
    const amountOut   = y - (x * y / (x + amountInFee));
    const fee         = amountIn - amountInFee;

    const spotBefore = (Number(y) / Number(x)).toFixed(2);
    const spotAfter  = (Number(y - amountOut) / Number(x + amountIn)).toFixed(2);
    const impact     = ((parseFloat(spotBefore) - parseFloat(spotAfter)) / parseFloat(spotBefore) * 100).toFixed(2);

    return {
      amountIn:    amountZbxStr,
      amountOut:   formatWei(amountOut),
      priceImpact: impact,
      spotBefore,
      spotAfter,
      fee:         formatWei(fee),
    };
  }

  /**
   * Quote a ZUSD-in swap (how much ZBX you get for X ZUSD).
   *
   * @example
   * const pool  = await zbx.amm.pool();
   * const quote = zbx.amm.quoteZusdIn("1000", pool);
   * console.log(`1000 ZUSD → \${quote.amountOut} ZBX`);
   */
  quoteZusdIn(amountZusdStr: string, pool: PoolState): SwapQuote {
    const amountIn = parseWei(amountZusdStr);
    const x = pool.zusdReserveWei;
    const y = pool.zbxReserveWei;

    if (x === 0n || y === 0n) throw new Error("Pool not initialized");

    const amountInFee = amountIn * (10000n - this.FEE_BPS) / 10000n;
    const amountOut   = y - (x * y / (x + amountInFee));
    const fee         = amountIn - amountInFee;

    const spotBefore = (Number(x) / Number(y)).toFixed(2);
    const spotAfter  = (Number(x + amountIn) / Number(y - amountOut)).toFixed(2);
    const impact     = ((parseFloat(spotAfter) - parseFloat(spotBefore)) / parseFloat(spotBefore) * 100).toFixed(2);

    return {
      amountIn:    amountZusdStr,
      amountOut:   formatWei(amountOut),
      priceImpact: impact,
      spotBefore,
      spotAfter,
      fee:         formatWei(fee),
    };
  }

  /**
   * Quote adding liquidity (how many LP tokens you get).
   *
   * @example
   * const pool  = await zbx.amm.pool();
   * const quote = zbx.amm.quoteLiquidity("1000", pool);
   * console.log("Add:", quote.zbxIn, "ZBX +", quote.zusdIn, "ZUSD");
   * console.log("Get:", quote.lpOut, "LP tokens");
   * console.log("Pool share:", quote.share, "%");
   */
  quoteLiquidity(amountZbxStr: string, pool: PoolState): LiquidityQuote {
    const zbxIn  = parseWei(amountZbxStr);
    const x = pool.zbxReserveWei;
    const y = pool.zusdReserveWei;
    const L = pool.lpSupply;

    if (x === 0n || y === 0n || L === 0n) {
      // First liquidity — use 1:1 initial price ($2500)
      const zusdIn = zbxIn * 2500n;
      return {
        zbxIn:  amountZbxStr,
        zusdIn: formatWei(zusdIn),
        lpOut:  formatWei(zbxIn), // initial LP = ZBX amount
        share:  "100.00",
      };
    }

    const zusdIn  = zbxIn * y / x;
    const lpOut   = zbxIn * L / x;
    const newL    = L + lpOut;
    const share   = (Number(lpOut) / Number(newL) * 100).toFixed(2);

    return {
      zbxIn:  amountZbxStr,
      zusdIn: formatWei(zusdIn),
      lpOut:  formatWei(lpOut),
      share,
    };
  }

  /**
   * Quote removing liquidity (how much ZBX + ZUSD you get back for LP tokens).
   *
   * @example
   * const pool  = await zbx.amm.pool();
   * const quote = zbx.amm.quoteRemoveLiquidity("100", pool);
   * console.log("Get:", quote.zbxOut, "ZBX +", quote.zusdOut, "ZUSD");
   */
  quoteRemoveLiquidity(lpAmountStr: string, pool: PoolState): { zbxOut: string; zusdOut: string; share: string } {
    const lpAmount = parseWei(lpAmountStr);
    const x = pool.zbxReserveWei;
    const y = pool.zusdReserveWei;
    const L = pool.lpSupply;

    if (L === 0n) throw new Error("Pool has no liquidity");

    const zbxOut  = lpAmount * x / L;
    const zusdOut = lpAmount * y / L;
    const share   = (Number(lpAmount) / Number(L) * 100).toFixed(2);

    return {
      zbxOut:  formatWei(zbxOut),
      zusdOut: formatWei(zusdOut),
      share,
    };
  }
}

function parseWei(amount: string): bigint {
  const [whole, frac = ""] = amount.split(".");
  return BigInt(whole) * 10n ** 18n + BigInt(frac.padEnd(18, "0").slice(0, 18) || "0");
}

function formatWei(wei: bigint, dec = 4): string {
  const whole = wei / 10n ** 18n;
  const frac  = wei % 10n ** 18n;
  const fracStr = frac.toString().padStart(18, "0").slice(0, dec).replace(/0+$/, "");
  return fracStr ? `\${whole}.\${fracStr}` : whole.toString();
}