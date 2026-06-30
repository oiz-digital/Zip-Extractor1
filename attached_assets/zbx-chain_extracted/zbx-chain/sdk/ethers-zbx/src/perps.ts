/**
 * Perps — ZbxPerpetuals v5 full wrapper for @zebvix/ethers (ZEP-034 rev4).
 * Accessible via `new Perps(providerOrSigner)`.
 *
 * Covers every public function in ZbxPerpetuals.sol:
 *   • Market views (getMarket, getMarkets, markPrice, fundingRate)
 *   • Position views (getPosition, healthBps, liqPrice, triggers, isLiq)
 *   • Cross-margin views (equity, maintMargin, freeMargin, positionIds, isCrossLiq)
 *   • Position writes (open, partialClose, close, addCollateral)
 *   • SL/TP/Trailing-stop writes
 *   • Keeper triggers (triggerOrder, triggerSL, triggerTP)
 *   • Liquidation (liquidate isolated, liquidateCross)
 *   • Cross account (depositCross, withdrawCross)
 *   • Funding (updateFunding)
 *   • Off-chain helpers (quotePnL, calcLiquidationPrice, validateOpen)
 *
 * @example
 * import { Perps, ZbxProvider, ZbxWallet } from "@zebvix/ethers";
 * import { ethers } from "ethers";
 *
 * const provider = new ZbxProvider();
 * const wallet   = new ZbxWallet(privateKey, provider);
 * const perps    = new Perps(wallet);   // signer for write helpers
 *
 * // ── Read market ───────────────────────────────────────────────────────────
 * const market = await perps.getMarket(0);
 * console.log("BTC price:", ethers.formatEther(market.markPrice), "USD");
 *
 * // ── Open 10× long (100 token collateral, isolated) ────────────────────────
 * // 1. approve collateralToken
 * // 2. send openPosition
 * const tx = await wallet.sendTransaction({
 *   to:   perps.address,
 *   data: perps.encodeOpenPosition({
 *     marketId:   0,
 *     isLong:     true,
 *     isCross:    false,
 *     collateral: ethers.parseEther("100"),
 *     leverage:   10,
 *     slPrice:    0n,
 *     tpPrice:    0n,
 *   }),
 * });
 *
 * // ── Read position with live PnL ───────────────────────────────────────────
 * const pos = await perps.getPosition(1);
 * console.log("PnL:", ethers.formatEther(pos.unrealisedPnl > 0n ? pos.unrealisedPnl : -pos.unrealisedPnl));
 * console.log("Health:", pos.healthBps.toString(), "bps");
 *
 * // ── Partial close 50% ─────────────────────────────────────────────────────
 * await wallet.sendTransaction({
 *   to:   perps.address,
 *   data: perps.encodePartialClose(1, 5000),
 * });
 *
 * // ── Set 2% trailing stop ──────────────────────────────────────────────────
 * await wallet.sendTransaction({
 *   to:   perps.address,
 *   data: perps.encodeSetTrailingStop(1, 200),
 * });
 *
 * // ── Keeper: scan and trigger ─────────────────────────────────────────────
 * const triggerable = await perps.scanPositions([1, 2, 3, 4, 5]);
 * for (const { positionId, reason } of triggerable) {
 *   if (reason === "liquidatable") {
 *     await wallet.sendTransaction({ to: perps.address, data: perps.encodeLiquidate(positionId) });
 *   } else {
 *     await wallet.sendTransaction({ to: perps.address, data: perps.encodeTriggerOrder(positionId) });
 *   }
 * }
 */
import { Contract, Interface, type Provider, type Signer } from "ethers";

// ─── Contract constants (matches ZbxPerpetuals.sol) ──────────────────────────

export const PERP_CONSTANTS = {
  MAX_LEVERAGE:           200n,
  MAINTENANCE_MARGIN_BPS: 1000n,  // 10%
  PROTOCOL_FEE_BPS:       10n,    // 0.10%
  KEEPER_BOUNTY_BPS:      5n,     // 0.05%
  LIQUIDATION_BOUNTY_BPS: 100n,   // 1.00%
  FUNDING_INTERVAL_SECS:  28800n, // 8 hours
  MAX_TRAIL_BPS:          5000n,  // 50%
  MAX_ORACLE_DELAY_SECS:  3600n,  // 1 hour (SEC-2026-05-09)
} as const;

// ─── ABI (matches ZbxPerpetuals.sol public interface) ────────────────────────

const ABI = [
  // ── State variables ─────────────────────────────────────────────────────
  "function owner() view returns (address)",
  "function collateralToken() view returns (address)",
  "function treasury() view returns (address)",
  "function marketCount() view returns (uint256)",
  "function nextPositionId() view returns (uint256)",
  "function protocolFeeBalance() view returns (uint256)",

  // ── Market views ────────────────────────────────────────────────────────
  "function getMarket(uint256 marketId) view returns (string symbol, address oracle, bool active, uint256 maxLeverage, uint256 totalLongOI, uint256 totalShortOI, int256 cumulativeFunding, uint256 nextFundingIn)",
  "function markPrice(uint256 marketId) view returns (uint256)",
  "function currentFundingRate(uint256 marketId) view returns (int256)",

  // ── Position views ───────────────────────────────────────────────────────
  // Full struct: trader, marketId, isLong, isCross, collateral, size, entryPrice,
  //              fundingEntryRate, stopLoss, takeProfit, trailBps, trailPeak,
  //              closed, initialMargin
  "function positions(uint256 positionId) view returns (address trader, uint256 marketId, bool isLong, bool isCross, uint256 collateral, uint256 size, uint256 entryPrice, int256 fundingEntryRate, uint256 stopLoss, uint256 takeProfit, uint256 trailBps, uint256 trailPeak, bool closed, uint256 initialMargin)",
  "function unrealisedPnl(uint256 positionId) view returns (int256)",
  "function healthBps(uint256 positionId) view returns (uint256)",
  "function liquidationPrice(uint256 positionId) view returns (uint256)",
  "function isLiquidatable(uint256 positionId) view returns (bool)",
  "function isSLTriggered(uint256 positionId) view returns (bool)",
  "function isTPTriggered(uint256 positionId) view returns (bool)",

  // ── Cross views ──────────────────────────────────────────────────────────
  "function crossBalance(address trader) view returns (uint256)",
  "function crossPositionIds(address trader) view returns (uint256[])",
  "function isCrossLiquidatable(address trader) view returns (bool)",
  "function crossEquity(address trader) view returns (int256)",
  "function crossMaintMargin(address trader) view returns (uint256)",
  "function freeCrossMargin(address trader) view returns (uint256)",
  "function crossLiquidationThreshold(address trader) view returns (uint256)",

  // ── Writes — position ───────────────────────────────────────────────────
  // Actual signature: openPosition(marketId, isLong, collateral, leverage, isCross, slPrice, tpPrice)
  // Size is computed on-chain: size = (collateral - fee) × leverage
  "function openPosition(uint256 marketId, bool isLong, uint256 collateral, uint256 leverage, bool isCross, uint256 slPrice, uint256 tpPrice) returns (uint256 positionId)",
  "function closePosition(uint256 positionId)",
  "function partialClose(uint256 positionId, uint256 closeBps)",
  "function addCollateral(uint256 positionId, uint256 amount)",

  // ── Writes — SL / TP / trailing stop ────────────────────────────────────
  "function setStopLoss(uint256 positionId, uint256 slPrice)",
  "function setTakeProfit(uint256 positionId, uint256 tpPrice)",
  "function setTrailingStop(uint256 positionId, uint256 trailBps)",
  "function updateTrailingStop(uint256 positionId)",

  // ── Writes — keeper triggers ─────────────────────────────────────────────
  "function triggerOrder(uint256 positionId)",
  "function triggerStopLoss(uint256 positionId)",
  "function triggerTakeProfit(uint256 positionId)",

  // ── Writes — liquidation ─────────────────────────────────────────────────
  "function liquidate(uint256 positionId)",
  "function liquidateCross(address trader)",

  // ── Writes — cross margin ────────────────────────────────────────────────
  "function depositCross(uint256 amount)",
  "function withdrawCross(uint256 amount)",

  // ── Writes — funding ─────────────────────────────────────────────────────
  "function updateFunding(uint256 marketId)",
] as const;

// ─── Types ───────────────────────────────────────────────────────────────────

export interface PerpMarket {
  marketId:        number;
  symbol:          string;
  oracle:          string;
  active:          boolean;
  /** Per-market leverage cap (≤ PERP_CONSTANTS.MAX_LEVERAGE). */
  maxLeverage:     bigint;
  totalLongOI:     bigint;
  totalShortOI:    bigint;
  /** Net OI imbalance: positive = more longs. */
  oiImbalance:     bigint;
  /** Cumulative funding rate (int256). */
  cumulativeFunding: bigint;
  /** Current 8-hour funding rate (int256). */
  currentFunding:  bigint;
  /** Seconds until next funding settlement. */
  nextFundingIn:   bigint;
  /** Current mark price (18-decimal, matches collateralToken decimals). */
  markPrice:       bigint;
}

export interface PerpPosition {
  positionId:          number;
  trader:              string;
  marketId:            bigint;
  isLong:              boolean;
  isCross:             boolean;
  /** Isolated collateral (0 for cross positions). */
  collateral:          bigint;
  /** Notional size = (collateral - fee) × leverage. */
  size:                bigint;
  entryPrice:          bigint;
  /** Cumulative funding rate at time of open (int256). */
  fundingEntryRate:    bigint;
  stopLoss:            bigint;
  takeProfit:          bigint;
  /** Trailing stop width in basis points (0 = disabled). */
  trailBps:            bigint;
  /** Highest (long) / lowest (short) mark price seen — trailing reference. */
  trailPeak:           bigint;
  closed:              boolean;
  /** Per-position initial margin share for cross IM accounting. */
  initialMargin:       bigint;
  // ── Live fields ──
  /** Unrealised PnL — signed int256. Positive = profit. */
  unrealisedPnl:       bigint;
  /** Position health in basis points (0 = liquidatable, 10000 = full collateral). */
  healthBps:           bigint;
  /** Oracle price at which this isolated position gets liquidated (0 for cross). */
  liquidationPrice:    bigint;
  /** Whether mark price has crossed the stop-loss level. */
  isSLTriggered:       boolean;
  /** Whether mark price has crossed the take-profit level. */
  isTPTriggered:       boolean;
  /** Whether the position is currently eligible for liquidation. */
  isLiquidatable:      boolean;
}

export interface CrossAccountState {
  trader:          string;
  balance:         bigint;
  /** balance + sum of unrealised PnLs across all cross positions (int256). */
  equity:          bigint;
  /** Sum of 10% maintenance margin across all open cross positions. */
  maintMargin:     bigint;
  /** Amount that can be freely withdrawn or used for new positions. */
  freeMargin:      bigint;
  /** Equity threshold below which the account gets liquidated (= maintMargin). */
  liqThreshold:    bigint;
  liquidatable:    boolean;
  positionIds:     bigint[];
}

export interface OpenPositionParams {
  marketId:   number;
  isLong:     boolean;
  /** true = cross account; false = isolated. */
  isCross:    boolean;
  /** Collateral in wei. Approve collateralToken before calling for isolated. */
  collateral: bigint;
  /** Leverage multiplier (1–maxLeverage). Size = (col-fee) × leverage. */
  leverage:   number;
  /** Stop-loss price in wei (0 = none). */
  slPrice:    bigint;
  /** Take-profit price in wei (0 = none). */
  tpPrice:    bigint;
}

/** Off-chain PnL estimate returned by quotePnL(). */
export interface PnlQuote {
  sizeWei:      bigint;
  entryPrice:   bigint;
  currentPrice: bigint;
  /** Signed PnL (negative = loss). */
  pnlWei:       bigint;
  /** PnL as basis points of notional (signed). */
  pnlBps:       bigint;
  /** ROE in basis points: pnl / collateral × 10000 (signed). */
  roeBps:       bigint;
}

// ─── Perps class ─────────────────────────────────────────────────────────────

export class Perps {
  readonly address: string;
  private readonly iface: Interface;
  private readonly contract: Contract;

  constructor(
    providerOrSigner: Provider | Signer,
    address = "0x000000000000000000000000005a425045525053",
  ) {
    this.address  = address;
    this.iface    = new Interface(ABI as unknown as string[]);
    this.contract = new Contract(address, ABI as unknown as string[], providerOrSigner);
  }

  // ══════════════════════════════════════════════════════════════════════════
  //  MARKET VIEWS
  // ══════════════════════════════════════════════════════════════════════════

  /**
   * Get full market info for a market ID including live mark price and funding rate.
   *
   * @example
   * const m = await perps.getMarket(0);
   * console.log("BTC price:", ethers.formatEther(m.markPrice), "USD");
   * console.log("Long OI:", ethers.formatEther(m.totalLongOI));
   */
  async getMarket(marketId: number): Promise<PerpMarket> {
    const [m, price, funding] = await Promise.all([
      this.contract.getMarket(marketId),
      this.contract.markPrice(marketId),
      this.contract.currentFundingRate(marketId),
    ]);
    const longOI  = m.totalLongOI  as bigint;
    const shortOI = m.totalShortOI as bigint;
    return {
      marketId,
      symbol:            m.symbol          as string,
      oracle:            m.oracle           as string,
      active:            m.active           as boolean,
      maxLeverage:       m.maxLeverage      as bigint,
      totalLongOI:       longOI,
      totalShortOI:      shortOI,
      oiImbalance:       longOI >= shortOI ? longOI - shortOI : shortOI - longOI,
      cumulativeFunding: m.cumulativeFunding as bigint,
      currentFunding:    funding            as bigint,
      nextFundingIn:     m.nextFundingIn    as bigint,
      markPrice:         price              as bigint,
    };
  }

  /**
   * Get all markets (parallel fetch).
   *
   * @example
   * const markets = await perps.getMarkets();
   * markets.forEach(m => console.log(m.symbol, ethers.formatEther(m.markPrice)));
   */
  async getMarkets(): Promise<PerpMarket[]> {
    const count = Number(await this.contract.marketCount());
    if (count === 0) return [];
    return Promise.all(Array.from({ length: count }, (_, i) => this.getMarket(i)));
  }

  /**
   * Get current mark price for a market (cheapest read — single call).
   *
   * @example
   * const price = await perps.getMarkPrice(0);
   * console.log("BTC:", ethers.formatEther(price), "USD");
   */
  async getMarkPrice(marketId: number): Promise<bigint> {
    return this.contract.markPrice(marketId) as Promise<bigint>;
  }

  /**
   * Get current 8-hour funding rate for a market (int256).
   * Positive = longs pay shorts; negative = shorts pay longs.
   */
  async getFundingRate(marketId: number): Promise<bigint> {
    return this.contract.currentFundingRate(marketId) as Promise<bigint>;
  }

  // ══════════════════════════════════════════════════════════════════════════
  //  POSITION VIEWS
  // ══════════════════════════════════════════════════════════════════════════

  /**
   * Get full position state with live PnL, health, and trigger status.
   * Fires 7 parallel calls for a complete snapshot.
   *
   * @example
   * const pos = await perps.getPosition(1);
   * const pnlEth = ethers.formatEther(pos.unrealisedPnl < 0n ? -pos.unrealisedPnl : pos.unrealisedPnl);
   * console.log("PnL:", pos.unrealisedPnl >= 0n ? "+" : "−", pnlEth);
   * console.log("Health:", pos.healthBps, "bps");
   * if (pos.isSLTriggered) console.log("SL triggered — call triggerOrder!");
   */
  async getPosition(positionId: number): Promise<PerpPosition> {
    const [p, pnl, health, liq, slHit, tpHit, isLiq] = await Promise.all([
      this.contract.positions(positionId),
      this.contract.unrealisedPnl(positionId),
      this.contract.healthBps(positionId),
      this.contract.liquidationPrice(positionId),
      this.contract.isSLTriggered(positionId),
      this.contract.isTPTriggered(positionId),
      this.contract.isLiquidatable(positionId),
    ]);
    return {
      positionId,
      trader:           p.trader           as string,
      marketId:         p.marketId         as bigint,
      isLong:           p.isLong           as boolean,
      isCross:          p.isCross          as boolean,
      collateral:       p.collateral       as bigint,
      size:             p.size             as bigint,
      entryPrice:       p.entryPrice       as bigint,
      fundingEntryRate: p.fundingEntryRate as bigint,
      stopLoss:         p.stopLoss         as bigint,
      takeProfit:       p.takeProfit       as bigint,
      trailBps:         p.trailBps         as bigint,
      trailPeak:        p.trailPeak        as bigint,
      closed:           p.closed           as boolean,
      initialMargin:    p.initialMargin    as bigint,
      unrealisedPnl:    pnl                as bigint,
      healthBps:        health             as bigint,
      liquidationPrice: liq                as bigint,
      isSLTriggered:    slHit              as boolean,
      isTPTriggered:    tpHit              as boolean,
      isLiquidatable:   isLiq              as boolean,
    };
  }

  /**
   * Get position health in basis points (cheap single call).
   * 0 = liquidatable, 10000 = fully margined.
   */
  async healthBps(positionId: number): Promise<bigint> {
    return this.contract.healthBps(positionId) as Promise<bigint>;
  }

  /** Check whether a position is currently liquidatable. */
  async isLiquidatable(positionId: number): Promise<boolean> {
    return this.contract.isLiquidatable(positionId) as Promise<boolean>;
  }

  /** Check whether the stop-loss price has been crossed by the mark price. */
  async isSLTriggered(positionId: number): Promise<boolean> {
    return this.contract.isSLTriggered(positionId) as Promise<boolean>;
  }

  /** Check whether the take-profit price has been crossed by the mark price. */
  async isTPTriggered(positionId: number): Promise<boolean> {
    return this.contract.isTPTriggered(positionId) as Promise<boolean>;
  }

  // ══════════════════════════════════════════════════════════════════════════
  //  CROSS-MARGIN VIEWS
  // ══════════════════════════════════════════════════════════════════════════

  /**
   * Get full cross-margin account state for a trader.
   *
   * @example
   * const acc = await perps.getCrossAccount("0xYourAddress");
   * console.log("Free margin:", ethers.formatEther(acc.freeMargin));
   * if (acc.liquidatable) console.warn("Cross account at liquidation risk!");
   */
  async getCrossAccount(trader: string): Promise<CrossAccountState> {
    const [bal, eq, maint, free, liqTh, liqBool, posIds] = await Promise.all([
      this.contract.crossBalance(trader),
      this.contract.crossEquity(trader),
      this.contract.crossMaintMargin(trader),
      this.contract.freeCrossMargin(trader),
      this.contract.crossLiquidationThreshold(trader),
      this.contract.isCrossLiquidatable(trader),
      this.contract.crossPositionIds(trader),
    ]);
    return {
      trader,
      balance:       bal      as bigint,
      equity:        eq       as bigint,
      maintMargin:   maint    as bigint,
      freeMargin:    free     as bigint,
      liqThreshold:  liqTh   as bigint,
      liquidatable:  liqBool  as boolean,
      positionIds:   posIds   as bigint[],
    };
  }

  /** Get free cross-margin available for withdrawal or new positions. */
  async freeCrossMargin(trader: string): Promise<bigint> {
    return this.contract.freeCrossMargin(trader) as Promise<bigint>;
  }

  /** Check if a cross account is currently liquidatable. */
  async isCrossLiquidatable(trader: string): Promise<boolean> {
    return this.contract.isCrossLiquidatable(trader) as Promise<boolean>;
  }

  /** Get IDs of all open cross-margin positions for a trader. */
  async crossPositionIds(trader: string): Promise<bigint[]> {
    return this.contract.crossPositionIds(trader) as Promise<bigint[]>;
  }

  // ══════════════════════════════════════════════════════════════════════════
  //  OFF-CHAIN COMPUTATION HELPERS (zero RPC calls)
  // ══════════════════════════════════════════════════════════════════════════

  /**
   * Estimate PnL given a current price — no RPC required.
   * Ignores funding (use for quick off-chain display).
   *
   * @example
   * const q = perps.quotePnL({
   *   isLong:       true,
   *   entryPrice:   ethers.parseEther("50000"),
   *   currentPrice: ethers.parseEther("52000"),
   *   size:         ethers.parseEther("10"),
   *   collateral:   ethers.parseEther("1"),
   * });
   * console.log("ROE:", q.roeBps.toString(), "bps");
   */
  quotePnL(params: {
    isLong:       boolean;
    entryPrice:   bigint;
    currentPrice: bigint;
    size:         bigint;
    collateral:   bigint;
  }): PnlQuote {
    const { isLong, entryPrice, currentPrice, size, collateral } = params;
    if (entryPrice === 0n) throw new Error("Perps.quotePnL: entryPrice cannot be 0");

    let pnlWei: bigint;
    if (isLong) {
      pnlWei = (currentPrice - entryPrice) * size / entryPrice;
    } else {
      pnlWei = (entryPrice - currentPrice) * size / entryPrice;
    }

    const absSize = size > 0n ? size : 1n;
    const absColl = collateral > 0n ? collateral : 1n;

    return {
      sizeWei:      size,
      entryPrice,
      currentPrice,
      pnlWei,
      pnlBps:  (pnlWei * 10_000n) / absSize,
      roeBps:  (pnlWei * 10_000n) / absColl,
    };
  }

  /**
   * Compute expected notional size from collateral and leverage — no RPC required.
   * Mirrors the contract: size = (collateral - protocolFee) × leverage
   *
   * @example
   * const size = perps.calcSize(ethers.parseEther("100"), 10);
   * console.log("Size:", ethers.formatEther(size));
   */
  calcSize(collateral: bigint, leverage: number): bigint {
    const fee    = (collateral * PERP_CONSTANTS.PROTOCOL_FEE_BPS) / 10_000n;
    const colNet = collateral - fee;
    return colNet * BigInt(leverage);
  }

  /**
   * Estimate liquidation price for an isolated position — no RPC required.
   * Matches the contract formula exactly (no funding component).
   *
   * @example
   * const liq = perps.calcLiquidationPrice({
   *   isLong:     true,
   *   entryPrice: ethers.parseEther("50000"),
   *   size:       ethers.parseEther("1000"),
   *   collateral: ethers.parseEther("100"),
   * });
   * console.log("Liq price:", ethers.formatEther(liq));
   */
  calcLiquidationPrice(params: {
    isLong:     boolean;
    entryPrice: bigint;
    size:       bigint;
    collateral: bigint;
  }): bigint {
    const { isLong, entryPrice, size, collateral } = params;
    if (size === 0n) throw new Error("Perps.calcLiquidationPrice: size cannot be 0");

    const MM       = (size * PERP_CONSTANTS.MAINTENANCE_MARGIN_BPS) / 10_000n;
    // numerator = MM - collateral (signed)
    const diff     = MM >= collateral ? MM - collateral : collateral - MM;
    const negative = MM < collateral;
    const delta    = (entryPrice * diff) / size;

    let liqPrice: bigint;
    if (isLong) {
      liqPrice = negative ? entryPrice - delta : entryPrice + delta;
    } else {
      liqPrice = negative ? entryPrice + delta : entryPrice - delta;
    }
    return liqPrice > 0n ? liqPrice : 0n;
  }

  /**
   * Validate open-position parameters client-side before submitting.
   * Returns null on success, or an error string on failure.
   * Fires a single `getMarket` RPC to check leverage and mark price.
   *
   * @example
   * const err = await perps.validateOpen({ marketId: 0, isLong: true, collateral: 100n, leverage: 10, isCross: false, slPrice: 0n, tpPrice: 0n });
   * if (err) throw new Error(err);
   */
  async validateOpen(params: OpenPositionParams): Promise<string | null> {
    if (params.collateral === 0n) return "collateral must be > 0";
    if (params.leverage < 1)     return "leverage must be >= 1";

    const market = await this.getMarket(params.marketId);
    if (!market.active) return `market ${params.marketId} is inactive`;
    if (BigInt(params.leverage) > market.maxLeverage) {
      return `leverage ${params.leverage} exceeds market max ${market.maxLeverage}`;
    }

    const mark = market.markPrice;
    if (params.slPrice > 0n) {
      if (params.isLong  && params.slPrice >= mark) return "long SL must be below mark price";
      if (!params.isLong && params.slPrice <= mark) return "short SL must be above mark price";
    }
    if (params.tpPrice > 0n) {
      if (params.isLong  && params.tpPrice <= mark) return "long TP must be above mark price";
      if (!params.isLong && params.tpPrice >= mark) return "short TP must be below mark price";
    }
    return null;
  }

  /**
   * Scan a list of position IDs for keeper opportunities (liquidations and SL/TP triggers).
   *
   * @example
   * const ops = await perps.scanPositions([1, 2, 3, 4, 5]);
   * for (const { positionId, reason } of ops) {
   *   console.log(`Position ${positionId}: ${reason}`);
   * }
   */
  async scanPositions(positionIds: number[]): Promise<Array<{
    positionId: number;
    reason: "liquidatable" | "sl_triggered" | "tp_triggered";
  }>> {
    const checks = await Promise.all(positionIds.map(async id => {
      const [liq, sl, tp] = await Promise.all([
        this.isLiquidatable(id),
        this.isSLTriggered(id),
        this.isTPTriggered(id),
      ]);
      return { positionId: id, liq, sl, tp };
    }));

    return checks
      .filter(c => c.liq || c.sl || c.tp)
      .map(c => ({
        positionId: c.positionId,
        reason: c.liq
          ? ("liquidatable" as const)
          : c.sl
          ? ("sl_triggered" as const)
          : ("tp_triggered" as const),
      }));
  }

  // ══════════════════════════════════════════════════════════════════════════
  //  CALLDATA ENCODERS — Position
  // ══════════════════════════════════════════════════════════════════════════

  /**
   * Encode `openPosition(marketId, isLong, collateral, leverage, isCross, slPrice, tpPrice)`.
   *
   * Contract computes size = (collateral - protocolFee) × leverage on-chain.
   * For isolated positions, approve collateralToken for `collateral` before calling.
   *
   * @example
   * const data = perps.encodeOpenPosition({
   *   marketId: 0, isLong: true, isCross: false,
   *   collateral: ethers.parseEther("100"), leverage: 10,
   *   slPrice: 0n, tpPrice: 0n,
   * });
   */
  encodeOpenPosition(p: OpenPositionParams): string {
    return this.iface.encodeFunctionData("openPosition", [
      p.marketId,
      p.isLong,
      p.collateral,
      p.leverage,
      p.isCross,
      p.slPrice,
      p.tpPrice,
    ]);
  }

  /** Encode `closePosition(uint256)` — fully close an isolated or cross position. */
  encodeClosePosition(positionId: number): string {
    return this.iface.encodeFunctionData("closePosition", [positionId]);
  }

  /**
   * Encode `partialClose(uint256 positionId, uint256 closeBps)`.
   * `closeBps` is basis points: 1–10000 (10000 = full close).
   *
   * @example
   * perps.encodePartialClose(1, 5000); // close 50%
   */
  encodePartialClose(positionId: number, closeBps: number): string {
    if (closeBps <= 0 || closeBps > 10_000) throw new Error("closeBps must be 1–10000");
    return this.iface.encodeFunctionData("partialClose", [positionId, closeBps]);
  }

  /**
   * Encode `addCollateral(uint256 positionId, uint256 amount)` (isolated only).
   * Approve collateralToken for `amount` before calling.
   */
  encodeAddCollateral(positionId: number, amountWei: bigint): string {
    return this.iface.encodeFunctionData("addCollateral", [positionId, amountWei]);
  }

  // ══════════════════════════════════════════════════════════════════════════
  //  CALLDATA ENCODERS — SL / TP / Trailing Stop
  // ══════════════════════════════════════════════════════════════════════════

  /**
   * Encode `setStopLoss(uint256 positionId, uint256 slPrice)`.
   * Pass 0n to remove the stop-loss.
   * Long: slPrice must be < markPrice. Short: slPrice must be > markPrice.
   */
  encodeSetStopLoss(positionId: number, slPrice: bigint): string {
    return this.iface.encodeFunctionData("setStopLoss", [positionId, slPrice]);
  }

  /**
   * Encode `setTakeProfit(uint256 positionId, uint256 tpPrice)`.
   * Pass 0n to remove the take-profit.
   * Long: tpPrice must be > markPrice. Short: tpPrice must be < markPrice.
   */
  encodeSetTakeProfit(positionId: number, tpPrice: bigint): string {
    return this.iface.encodeFunctionData("setTakeProfit", [positionId, tpPrice]);
  }

  /**
   * Encode `setTrailingStop(uint256 positionId, uint256 trailBps)`.
   * `trailBps` = trail width in basis points (1–5000).
   * Overwrites any existing static stop-loss.
   * The SL ratchets upward (long) or downward (short) as price improves.
   *
   * @example
   * perps.encodeSetTrailingStop(1, 200); // 2% trail
   */
  encodeSetTrailingStop(positionId: number, trailBps: number): string {
    if (trailBps <= 0 || trailBps > 5000) throw new Error("trailBps must be 1–5000");
    return this.iface.encodeFunctionData("setTrailingStop", [positionId, trailBps]);
  }

  /**
   * Encode `updateTrailingStop(uint256 positionId)`.
   * Keeper ratchets the trailing SL when mark price has improved past the last peak.
   * Reverts with `TrailNotFavourable` if price has not moved favourably.
   */
  encodeUpdateTrailingStop(positionId: number): string {
    return this.iface.encodeFunctionData("updateTrailingStop", [positionId]);
  }

  // ══════════════════════════════════════════════════════════════════════════
  //  CALLDATA ENCODERS — Keeper Triggers
  // ══════════════════════════════════════════════════════════════════════════

  /**
   * Encode `triggerOrder(uint256 positionId)`.
   * Unified trigger — executes whichever of SL or TP is currently hit.
   * Earns 0.05% KEEPER_BOUNTY. Reverts with `NeitherTriggered` if neither active.
   *
   * Prefer this over encodeTriggerStopLoss / encodeTriggerTakeProfit for keeper bots.
   */
  encodeTriggerOrder(positionId: number): string {
    return this.iface.encodeFunctionData("triggerOrder", [positionId]);
  }

  /**
   * Encode `triggerStopLoss(uint256 positionId)`.
   * Reverts with `SLNotTriggered` if mark price has not crossed the SL.
   */
  encodeTriggerStopLoss(positionId: number): string {
    return this.iface.encodeFunctionData("triggerStopLoss", [positionId]);
  }

  /**
   * Encode `triggerTakeProfit(uint256 positionId)`.
   * Reverts with `TPNotTriggered` if mark price has not crossed the TP.
   */
  encodeTriggerTakeProfit(positionId: number): string {
    return this.iface.encodeFunctionData("triggerTakeProfit", [positionId]);
  }

  // ══════════════════════════════════════════════════════════════════════════
  //  CALLDATA ENCODERS — Liquidation
  // ══════════════════════════════════════════════════════════════════════════

  /**
   * Encode `liquidate(uint256 positionId)` — liquidate an isolated position.
   * Earns 1% LIQUIDATION_BOUNTY of the position's collateral.
   */
  encodeLiquidate(positionId: number): string {
    return this.iface.encodeFunctionData("liquidate", [positionId]);
  }

  /**
   * Encode `liquidateCross(address trader)` — liquidate an entire cross account.
   * Earns 1% LIQUIDATION_BOUNTY of the cross account balance.
   * Reverts with `NotLiquidatable` if cross equity > maint margin.
   */
  encodeLiquidateCross(trader: string): string {
    return this.iface.encodeFunctionData("liquidateCross", [trader]);
  }

  // ══════════════════════════════════════════════════════════════════════════
  //  CALLDATA ENCODERS — Cross Margin
  // ══════════════════════════════════════════════════════════════════════════

  /**
   * Encode `depositCross(uint256 amount)`.
   * Approve collateralToken for `amount` before calling.
   */
  encodeDepositCross(amountWei: bigint): string {
    return this.iface.encodeFunctionData("depositCross", [amountWei]);
  }

  /**
   * Encode `withdrawCross(uint256 amount)`.
   * Reverts with `CrossWithdrawTooLarge` if amount > freeCrossMargin.
   */
  encodeWithdrawCross(amountWei: bigint): string {
    return this.iface.encodeFunctionData("withdrawCross", [amountWei]);
  }

  // ══════════════════════════════════════════════════════════════════════════
  //  CALLDATA ENCODERS — Funding
  // ══════════════════════════════════════════════════════════════════════════

  /**
   * Encode `updateFunding(uint256 marketId)`.
   * Anyone can call this to settle an overdue funding epoch.
   * The update also fires automatically on any trade.
   */
  encodeUpdateFunding(marketId: number): string {
    return this.iface.encodeFunctionData("updateFunding", [marketId]);
  }
}
