/**
 * VaultHelper — ZusdVault CDP (Collateralised Debt Position) operations.
 * Accessible via `client.vault.*`
 *
 * Mirrors MakerDAO-style CDPs: lock ZBX → mint ZUSD (up to 50% of collateral
 * value). If ZBX price drops 50%, position becomes liquidatable.
 *
 * Contract: ZusdVault.sol — ZEP-012
 *
 * @example
 * import { ZbxClient } from "zebvix.js";
 *
 * const zbx    = new ZbxClient("https://rpc.zebvix.io");
 * const wallet = zbx.wallet(process.env.PRIVATE_KEY!);
 *
 * // Open a CDP: lock 1000 ZBX as collateral
 * const hash = await wallet.vault.openCDP("1000");
 *
 * // Mint 1000 ZUSD against the collateral
 * const mintHash = await wallet.vault.mintMore("1000");
 *
 * // Check your CDP health
 * const cdp = await zbx.vault.getCDP("0xYourAddress");
 * console.log("Collateral ratio:", cdp.crPercent, "%");
 * console.log("ZUSD debt:", cdp.debt, "ZUSD");
 *
 * // Repay some ZUSD
 * await wallet.vault.repay("500");
 */
import type { ZbxClient } from "./client";

// ─── Types ───────────────────────────────────────────────────────────────────

export interface CDP {
  /** ZBX collateral locked (wei). */
  collateralWei:  bigint;
  /** ZBX collateral (formatted string). */
  collateral:     string;
  /** ZUSD principal debt snapshot (wei, without accrued fees). */
  debtWei:        bigint;
  /** ZUSD debt (formatted string). */
  debt:           string;
  /** Live current debt including accrued stability fees (wei). */
  currentDebtWei: bigint;
  /** Live current debt (formatted string). */
  currentDebt:    string;
  /** Collateral ratio in basis points (e.g. 25000 = 250%). */
  crBps:          number;
  /** Collateral ratio as percentage string (e.g. "250.00"). */
  crPercent:      string;
  /** Whether the position is currently liquidatable (CR ≤ 100%). */
  liquidatable:   boolean;
  /** Whether the CDP exists (collateral > 0). */
  exists:         boolean;
}

export interface VaultStats {
  /** Total ZBX locked in vault (wei). */
  totalCollateralWei: bigint;
  totalCollateral:    string;
  /** Total ZUSD minted (wei). */
  totalDebtWei:       bigint;
  totalDebt:          string;
  /** System-wide collateral ratio in bps. */
  systemCrBps:        number;
  systemCr:           string;
  /** Stability fee APY in bps (e.g. 200 = 2%). */
  stabilityFeeBps:    number;
  /** Stability fee APY as percentage string. */
  stabilityFee:       string;
}

export interface MintQuote {
  /** Maximum ZUSD mintable for given collateral at current price (wei). */
  maxMintableWei: bigint;
  maxMintable:    string;
  /** ZBX price used for the quote (USD, 18-decimal). */
  zbxPriceUsd:    string;
  /** Resulting collateral ratio if you mint this.maxMintable (bps). */
  minCrBps:       number;
}

// ─── ABI selectors ───────────────────────────────────────────────────────────

const SEL = {
  openCDP:       "0x4a39b63c", // openCDP(uint256 collateralAmount)
  mintMore:      "0x7f3e2a3a", // mintMore(uint256 zusdAmount)
  repay:         "0x371fd8e6", // repay(uint256 zusdAmount)
  closeCDP:      "0x7e4e5b47", // closeCDP()
  addCollateral: "0x15c5f231", // addCollateral(uint256 amount)
  liquidate:     "0x8d1fdf2f", // liquidate(address cdpOwner)
  cdps:          "0x9355f74e", // cdps(address) → (uint256 collateral, uint256 debt, uint256 lastFeeIndex)
  currentDebt:   "0xdbace0d5", // currentDebt(address) — external wrapper
  collateralRatio: "0x1d59fbc6", // collateralRatio(address) → (uint256 crBps, uint256 debt)
  maxMintable:   "0xf9ef7f17", // maxMintableZusd(uint256 collateral) → uint256
  totalCollateral: "0xb64e39af", // totalCollateral()
  totalDebt:     "0xfc7e286d", // totalDebt()
  stabilityFeeBps: "0x24db5e21", // stabilityFeeBps() — constant getter
} as const;

// ─── Helper ──────────────────────────────────────────────────────────────────

export class VaultHelper {
  constructor(
    private readonly client: ZbxClient,
    readonly contractAddress: string = "0x000000000000000000000000005a425641554c54",
  ) {}

  // ── Views ──────────────────────────────────────────────────────────────────

  /**
   * Get full CDP state for an address.
   *
   * @example
   * const cdp = await zbx.vault.getCDP("0xYourAddress");
   * if (!cdp.exists) console.log("No CDP");
   * else console.log("Collateral ratio:", cdp.crPercent, "%");
   */
  async getCDP(owner: string): Promise<CDP> {
    const addrPad = owner.slice(2).padStart(64, "0");

    const [rawCdp, rawCr] = await Promise.all([
      this._call(SEL.cdps + addrPad),
      this._call(SEL.collateralRatio + addrPad),
    ]);

    const b   = rawCdp.replace(/^0x/, "");
    const colWei  = h2b(b.slice(0, 64));
    const debtWei = h2b(b.slice(64, 128));

    const crBuf       = rawCr.replace(/^0x/, "");
    const crBps       = Number(h2b(crBuf.slice(0, 64)));
    const currentDebtWei = h2b(crBuf.slice(64, 128));

    const exists      = colWei > 0n;
    const liquidatable = crBps > 0 && crBps <= 10_000; // ≤ 100% CR

    return {
      collateralWei:  colWei,
      collateral:     fmtWei(colWei),
      debtWei,
      debt:           fmtWei(debtWei),
      currentDebtWei,
      currentDebt:    fmtWei(currentDebtWei),
      crBps,
      crPercent:      crBps > 0 ? (crBps / 100).toFixed(2) : "∞",
      liquidatable,
      exists,
    };
  }

  /**
   * Quote the maximum ZUSD mintable for a given ZBX collateral amount.
   *
   * @example
   * const quote = await zbx.vault.quoteMint("1000");
   * console.log("Max ZUSD:", quote.maxMintable);
   * console.log("At price:", quote.zbxPriceUsd, "USD/ZBX");
   */
  async quoteMint(collateralZbx: string): Promise<MintQuote> {
    const colWei = pWei(collateralZbx);
    const raw    = await this._call(
      SEL.maxMintable + colWei.toString(16).padStart(64, "0"),
    );
    const maxWei = h2b(raw.replace(/^0x/, "").slice(0, 64));

    // Infer price: maxMintable = collateral * price / 2 (50% MCR)
    const impliedPriceWei = maxWei > 0n ? (maxWei * 2n * 10n ** 18n) / colWei : 0n;

    return {
      maxMintableWei: maxWei,
      maxMintable:    fmtWei(maxWei, 2),
      zbxPriceUsd:    fmtWei(impliedPriceWei, 2),
      minCrBps:       20_000, // 200% = minimum safe CR
    };
  }

  /**
   * Get global vault statistics.
   *
   * @example
   * const stats = await zbx.vault.getStats();
   * console.log("Total ZBX locked:", stats.totalCollateral);
   * console.log("Total ZUSD minted:", stats.totalDebt);
   */
  async getStats(): Promise<VaultStats> {
    const [rawCol, rawDebt, rawFee] = await Promise.all([
      this._call(SEL.totalCollateral),
      this._call(SEL.totalDebt),
      this._call(SEL.stabilityFeeBps),
    ]);

    const colWei  = h2b(rawCol.replace(/^0x/, "").slice(0, 64));
    const debtWei = h2b(rawDebt.replace(/^0x/, "").slice(0, 64));
    const feeBps  = Number(h2b(rawFee.replace(/^0x/, "").slice(0, 64)));

    const systemCrBps = debtWei > 0n
      ? Number((colWei * 10_000n) / debtWei)
      : 999_999;

    return {
      totalCollateralWei: colWei,
      totalCollateral:    fmtWei(colWei),
      totalDebtWei:       debtWei,
      totalDebt:          fmtWei(debtWei, 2),
      systemCrBps,
      systemCr:           (systemCrBps / 100).toFixed(2),
      stabilityFeeBps:    feeBps,
      stabilityFee:       (feeBps / 100).toFixed(2),
    };
  }

  // ── Transaction encoders ───────────────────────────────────────────────────

  /** Encode `openCDP(uint256)` calldata — lock ZBX collateral (send as value). */
  encodeOpenCDP(collateralZbx: string): string {
    return SEL.openCDP + pWei(collateralZbx).toString(16).padStart(64, "0");
  }

  /** Encode `mintMore(uint256)` calldata — mint additional ZUSD. */
  encodeMintMore(zusdAmount: string): string {
    return SEL.mintMore + pWei(zusdAmount).toString(16).padStart(64, "0");
  }

  /** Encode `repay(uint256)` calldata — repay ZUSD debt. */
  encodeRepay(zusdAmount: string): string {
    return SEL.repay + pWei(zusdAmount).toString(16).padStart(64, "0");
  }

  /** Encode `addCollateral(uint256)` calldata — add more ZBX collateral. */
  encodeAddCollateral(collateralZbx: string): string {
    return SEL.addCollateral + pWei(collateralZbx).toString(16).padStart(64, "0");
  }

  /** Encode `closeCDP()` calldata — repay all debt and unlock all collateral. */
  encodeCloseCDP(): string {
    return SEL.closeCDP;
  }

  /** Encode `liquidate(address)` calldata — liquidate an undercollateralised CDP. */
  encodeLiquidate(cdpOwner: string): string {
    return SEL.liquidate + cdpOwner.slice(2).padStart(64, "0");
  }

  // ── Private ────────────────────────────────────────────────────────────────

  private async _call(data: string): Promise<string> {
    return this.client.rpc<string>("eth_call", [
      { to: this.contractAddress, data },
      "latest",
    ]);
  }
}

// ── Utilities ─────────────────────────────────────────────────────────────────

function h2b(hex: string): bigint {
  return hex ? BigInt("0x" + hex) : 0n;
}

function fmtWei(wei: bigint, dec = 4): string {
  const whole  = wei / 10n ** 18n;
  const frac   = wei % 10n ** 18n;
  const fracStr = frac.toString().padStart(18, "0").slice(0, dec).replace(/0+$/, "");
  return fracStr ? `${whole}.${fracStr}` : whole.toString();
}

function pWei(amount: string): bigint {
  const [w, f = ""] = amount.split(".");
  return BigInt(w) * 10n ** 18n + BigInt(f.padEnd(18, "0").slice(0, 18) || "0");
}
