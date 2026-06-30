/**
 * LendingHelper — ZbxLendingPool supply/borrow/repay/liquidate interaction.
 * Accessible via `client.lending.*`
 *
 * Aave-style money market: supply collateral to earn yield, borrow against it.
 * Health factor < 1.0 triggers liquidation (up to 50% close factor per call).
 *
 * Contract: ZbxLendingPool.sol — ZEP-031
 *
 * @example
 * import { ZbxClient } from "zebvix.js";
 *
 * const zbx    = new ZbxClient("https://rpc.zebvix.io");
 * const wallet = zbx.wallet(process.env.PRIVATE_KEY!);
 *
 * // Check your account health
 * const acc = await zbx.lending.getAccountData("0xYourAddress");
 * console.log("Health factor:", acc.healthFactor);
 * console.log("Available to borrow:", acc.availableBorrowUsd, "USD");
 *
 * // Supply 1000 ZUSD
 * const data = zbx.lending.encodeSupply("0xZUSD", "1000");
 * await wallet.sendTx({ to: zbx.lending.contractAddress, data });
 *
 * // Borrow 500 ZUSD against supplied collateral
 * const borrowData = zbx.lending.encodeBorrow("0xZUSD", "500");
 * await wallet.sendTx({ to: zbx.lending.contractAddress, data: borrowData });
 */
import type { ZbxClient } from "./client";

// ─── Types ───────────────────────────────────────────────────────────────────

export interface AccountData {
  /** Total collateral value in USD (18-decimal string). */
  totalCollateralUsd:   string;
  totalCollateralUsdWei: bigint;
  /** Total debt value in USD (18-decimal string). */
  totalDebtUsd:         string;
  totalDebtUsdWei:      bigint;
  /** Available to borrow in USD. */
  availableBorrowUsd:   string;
  /** Current liquidation threshold in basis points. */
  liquidationThresholdBps: number;
  /** Loan-to-value ratio in basis points. */
  ltvBps:               number;
  /**
   * Health factor (18-decimal bigint).
   * < 1e18 = liquidatable.
   * 0 when totalDebt == 0 and totalCollateral == 0.
   */
  healthFactorWei:      bigint;
  /** Health factor as a human-readable string (e.g. "1.42"). */
  healthFactor:         string;
  /** Whether the account is currently liquidatable. */
  liquidatable:         boolean;
}

export interface MarketData {
  token:               string;
  /** Total tokens supplied to the pool (formatted). */
  totalSupply:         string;
  totalSupplyWei:      bigint;
  /** Total tokens borrowed from the pool (formatted). */
  totalBorrow:         string;
  totalBorrowWei:      bigint;
  /** Utilisation ratio as a percentage string (e.g. "72.5"). */
  utilisation:         string;
  /** Current supply APY (basis points). */
  supplyApyBps:        number;
  /** Current borrow APY (basis points). */
  borrowApyBps:        number;
  /** Collateral factor in basis points (e.g. 7500 = 75% LTV). */
  collateralFactorBps: number;
  active:              boolean;
}

// ─── ABI selectors ───────────────────────────────────────────────────────────

const SEL = {
  supply:             "0xf7a73840", // supply(address token, uint256 amount)
  withdraw:           "0x441a3e70", // withdraw(address token, uint256 amount)
  borrow:             "0xc858f5f9", // borrow(address token, uint256 amount)
  repay:              "0x5ceae9c4", // repay(address token, uint256 amount)
  liquidate:          "0x96cd97db", // liquidate(address borrower, address collateralToken, uint256 repayAmount, bool receiveAToken)
  getUserAccountData: "0xbf92857c", // getUserAccountData(address) → AccountDataResult
  getMarketData:      "0x3d3d2fd9", // getMarketData(address token) → MarketData
  getSupplyBalance:   "0x4f2be91f", // getSupplyBalance(address user, address token) → uint256
  getBorrowBalance:   "0x56e4026d", // getBorrowBalance(address user, address token) → uint256
  paused:             "0x5c975abb", // paused()
} as const;

// ─── Helper ──────────────────────────────────────────────────────────────────

export class LendingHelper {
  constructor(
    private readonly client: ZbxClient,
    readonly contractAddress: string = "0x000000000000000000000000005a424c454e4421",
  ) {}

  // ── Views ──────────────────────────────────────────────────────────────────

  /**
   * Get full account health data.
   *
   * @example
   * const acc = await zbx.lending.getAccountData("0xYourAddress");
   * if (acc.liquidatable) console.warn("Account is undercollateralised!");
   * console.log("Health factor:", acc.healthFactor);
   */
  async getAccountData(user: string): Promise<AccountData> {
    const raw = await this._call(SEL.getUserAccountData + user.slice(2).padStart(64, "0"));
    const b   = raw.replace(/^0x/, "");

    const colWei  = h2b(b.slice(0, 64));
    const debtWei = h2b(b.slice(64, 128));
    const availWei= h2b(b.slice(128, 192));
    const ltBps   = Number(h2b(b.slice(192, 256)));
    const ltvBps  = Number(h2b(b.slice(256, 320)));
    const hfWei   = h2b(b.slice(320, 384));

    const hfNum   = debtWei === 0n ? Infinity : Number(hfWei) / 1e18;
    const hfStr   = hfNum === Infinity ? "∞" : hfNum.toFixed(2);

    return {
      totalCollateralUsd:      fmtWei(colWei, 2),
      totalCollateralUsdWei:   colWei,
      totalDebtUsd:            fmtWei(debtWei, 2),
      totalDebtUsdWei:         debtWei,
      availableBorrowUsd:      fmtWei(availWei, 2),
      liquidationThresholdBps: ltBps,
      ltvBps,
      healthFactorWei:         hfWei,
      healthFactor:            hfStr,
      liquidatable:            debtWei > 0n && hfWei < 10n ** 18n,
    };
  }

  /**
   * Get market data for a specific token.
   *
   * @example
   * const mkt = await zbx.lending.getMarketData("0xZUSD");
   * console.log("Supply APY:", mkt.supplyApyBps / 100, "%");
   * console.log("Utilisation:", mkt.utilisation, "%");
   */
  async getMarketData(tokenAddress: string): Promise<MarketData> {
    const raw = await this._call(SEL.getMarketData + tokenAddress.slice(2).padStart(64, "0"));
    const b   = raw.replace(/^0x/, "");

    const totalSupplyWei = h2b(b.slice(0, 64));
    const totalBorrowWei = h2b(b.slice(64, 128));
    const supplyApyBps   = Number(h2b(b.slice(128, 192)));
    const borrowApyBps   = Number(h2b(b.slice(192, 256)));
    const cfBps          = Number(h2b(b.slice(256, 320)));
    const active         = h2b(b.slice(320, 384)) !== 0n;

    const util = totalSupplyWei > 0n
      ? (Number(totalBorrowWei * 10_000n / totalSupplyWei) / 100).toFixed(1)
      : "0.0";

    return {
      token:               tokenAddress,
      totalSupply:         fmtWei(totalSupplyWei),
      totalSupplyWei,
      totalBorrow:         fmtWei(totalBorrowWei),
      totalBorrowWei,
      utilisation:         util,
      supplyApyBps,
      borrowApyBps,
      collateralFactorBps: cfBps,
      active,
    };
  }

  /**
   * Get a user's supply balance for a specific token.
   *
   * @example
   * const bal = await zbx.lending.getSupplyBalance("0xYourAddress", "0xZUSD");
   * console.log("Supplied ZUSD:", fmtWei(bal));
   */
  async getSupplyBalance(user: string, token: string): Promise<bigint> {
    const data = SEL.getSupplyBalance
      + user.slice(2).padStart(64, "0")
      + token.slice(2).padStart(64, "0");
    const raw = await this._call(data);
    return h2b(raw.replace(/^0x/, "").slice(0, 64));
  }

  /**
   * Get a user's borrow balance for a specific token (includes accrued interest).
   *
   * @example
   * const debt = await zbx.lending.getBorrowBalance("0xYourAddress", "0xZUSD");
   * console.log("Owed ZUSD:", fmtWei(debt));
   */
  async getBorrowBalance(user: string, token: string): Promise<bigint> {
    const data = SEL.getBorrowBalance
      + user.slice(2).padStart(64, "0")
      + token.slice(2).padStart(64, "0");
    const raw = await this._call(data);
    return h2b(raw.replace(/^0x/, "").slice(0, 64));
  }

  // ── Transaction encoders ───────────────────────────────────────────────────

  /** Encode `supply(address token, uint256 amount)` calldata. */
  encodeSupply(tokenAddress: string, amount: string): string {
    return SEL.supply
      + tokenAddress.slice(2).padStart(64, "0")
      + pWei(amount).toString(16).padStart(64, "0");
  }

  /** Encode `withdraw(address token, uint256 amount)` calldata. */
  encodeWithdraw(tokenAddress: string, amount: string): string {
    return SEL.withdraw
      + tokenAddress.slice(2).padStart(64, "0")
      + pWei(amount).toString(16).padStart(64, "0");
  }

  /** Encode `borrow(address token, uint256 amount)` calldata. */
  encodeBorrow(tokenAddress: string, amount: string): string {
    return SEL.borrow
      + tokenAddress.slice(2).padStart(64, "0")
      + pWei(amount).toString(16).padStart(64, "0");
  }

  /** Encode `repay(address token, uint256 amount)` calldata. */
  encodeRepay(tokenAddress: string, amount: string): string {
    return SEL.repay
      + tokenAddress.slice(2).padStart(64, "0")
      + pWei(amount).toString(16).padStart(64, "0");
  }

  /**
   * Encode `liquidate(...)` calldata.
   * Caller must hold `repayAmount` of `collateralToken` and have approved the pool.
   *
   * @example
   * // Liquidate 50% of borrower's ZUSD debt, receive ZBX collateral
   * const data = zbx.lending.encodeLiquidate({
   *   borrower:        "0xBorrower",
   *   collateralToken: "0xZUSD",
   *   repayAmount:     "500",
   *   receiveAToken:   false,
   * });
   */
  encodeLiquidate(params: {
    borrower:        string;
    collateralToken: string;
    repayAmount:     string;
    receiveAToken:   boolean;
  }): string {
    return SEL.liquidate
      + params.borrower.slice(2).padStart(64, "0")
      + params.collateralToken.slice(2).padStart(64, "0")
      + pWei(params.repayAmount).toString(16).padStart(64, "0")
      + (params.receiveAToken ? "1" : "0").padStart(64, "0");
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
