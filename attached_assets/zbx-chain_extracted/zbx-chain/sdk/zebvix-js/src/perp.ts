/**
 * PerpHelper — ZbxPerpetuals v5 full SDK (ZEP-034 rev4).
 * Accessible via `client.perp.*`
 *
 * Covers every public function in ZbxPerpetuals.sol:
 *   • Market management views
 *   • Position open / partial-close / full-close
 *   • SL / TP / Trailing-stop set & update
 *   • Keeper trigger (SL, TP, combined triggerOrder)
 *   • Isolated & cross-margin liquidation
 *   • Cross account deposit / withdraw / equity views
 *   • Funding-rate views and manual updateFunding
 *   • Off-chain PnL / liquidation-price / validation helpers
 *
 * Contract: ZbxPerpetuals.sol — ZEP-034 rev4 (max leverage 200×)
 *
 * @example
 * import { ZbxClient } from "zebvix.js";
 *
 * const zbx    = new ZbxClient("https://rpc.zebvix.io");
 * const wallet = zbx.wallet(process.env.PRIVATE_KEY!);
 *
 * // ── Open a 10× long on BTC-USD (isolated, 100-token margin) ────────────────
 * const openData = zbx.perp.encodeOpenPosition({
 *   marketId:   0,
 *   isLong:     true,
 *   isCross:    false,
 *   collateral: zbx.parseZbx("100"),   // margin deposited
 *   leverage:   10,                    // size = (collateral - fee) * leverage
 *   slPrice:    0n,                    // no stop-loss
 *   tpPrice:    0n,                    // no take-profit
 * });
 * const hash = await wallet.sendTx({ to: zbx.perp.contractAddress, data: openData });
 *
 * // ── Monitor position health ────────────────────────────────────────────────
 * const pos = await zbx.perp.getPosition(1);
 * if (pos.healthBps < 500) console.warn("Near liquidation!");
 * if (pos.isSLTriggered) console.log("Stop-loss ready to trigger");
 *
 * // ── Partial close 50% of position ─────────────────────────────────────────
 * await wallet.sendTx({
 *   to:   zbx.perp.contractAddress,
 *   data: zbx.perp.encodePartialClose(1, 5000), // 5000 bps = 50%
 * });
 *
 * // ── Set a trailing stop (2% trail) ─────────────────────────────────────────
 * await wallet.sendTx({
 *   to:   zbx.perp.contractAddress,
 *   data: zbx.perp.encodeSetTrailingStop(1, 200), // 200 bps = 2%
 * });
 *
 * // ── Cross-margin account ───────────────────────────────────────────────────
 * const cross = await zbx.perp.getCrossAccount("0xYourAddress");
 * console.log("Cross equity:", cross.equity);
 * console.log("Free margin:", cross.freeMargin);
 * if (cross.liquidatable) console.warn("Cross account liquidatable!");
 */
import type { ZbxClient } from "./client";
import { keccak_256 } from "@noble/hashes/sha3";

// ─── Contract constants (matches ZbxPerpetuals.sol) ──────────────────────────

export const PERP_CONSTANTS = {
  MAX_LEVERAGE:           200n,
  MAINTENANCE_MARGIN_BPS: 1000n,  // 10%
  PROTOCOL_FEE_BPS:       10n,    // 0.10%
  KEEPER_BOUNTY_BPS:      5n,     // 0.05%
  LIQUIDATION_BOUNTY_BPS: 100n,   // 1.00%
  FUNDING_INTERVAL_SECS:  28800n, // 8 hours
  MAX_TRAIL_BPS:          5000n,  // 50%
  MAX_ORACLE_DELAY_SECS:  3600n,  // 1 hour
} as const;

// ─── Types ───────────────────────────────────────────────────────────────────

export interface MarketInfo {
  marketId:        number;
  symbol:          string;
  oracle:          string;
  active:          boolean;
  maxLeverage:     number;
  totalLongOI:     string;
  totalLongOIWei:  bigint;
  totalShortOI:    string;
  totalShortOIWei: bigint;
  /** Net OI imbalance: positive = more longs. */
  oiImbalance:     string;
  /** Cumulative funding rate (signed, raw). */
  fundingRate:     string;
  /** Seconds until next 8-hour funding settlement (0 if overdue). */
  nextFundingIn:   number;
  /** Current mark price (formatted, 2 decimals). */
  markPrice:       string;
  markPriceWei:    bigint;
}

export interface Position {
  positionId:         number;
  trader:             string;
  marketId:           number;
  isLong:             boolean;
  isCross:            boolean;
  /** Isolated collateral. 0 for cross positions (use cross account balance). */
  collateral:         string;
  collateralWei:      bigint;
  /** Notional size = (collateral - fee) × leverage. */
  size:               string;
  sizeWei:            bigint;
  entryPrice:         string;
  entryPriceWei:      bigint;
  /** Accrued funding at time of open (int256). */
  fundingEntryRate:   string;
  stopLoss:           string;
  stopLossWei:        bigint;
  takeProfit:         string;
  takeProfitWei:      bigint;
  /** Trailing stop trail width in basis points (0 = no trailing stop). */
  trailBps:           number;
  /** Highest (long) or lowest (short) mark price seen — trailing stop reference. */
  trailPeak:          string;
  trailPeakWei:       bigint;
  closed:             boolean;
  /** Per-position initial margin share (used for cross IM accounting). */
  initialMargin:      string;
  initialMarginWei:   bigint;
  // ── Live fields (populated by getPosition) ──
  /** Unrealised PnL (signed string, e.g. "+123.45" or "−50.00"). */
  unrealisedPnl:      string;
  unrealisedPnlWei:   bigint;
  unrealisedPnlSign:  "+" | "−";
  /** Health in basis points. 0 = liquidatable, 10000 = full collateral. */
  healthBps:          number;
  /** Exact mark price at which this position gets liquidated (0 for cross). */
  liquidationPrice:   string;
  liquidationPriceWei: bigint;
  /** Whether the current mark price has crossed the stop-loss level. */
  isSLTriggered:      boolean;
  /** Whether the current mark price has crossed the take-profit level. */
  isTPTriggered:      boolean;
  /** Whether the position is currently liquidatable. */
  isLiquidatable:     boolean;
}

export interface CrossAccountState {
  trader:          string;
  balance:         string;
  balanceWei:      bigint;
  /** balance + sum of unrealised PnLs across all cross positions. */
  equity:          string;
  equityWei:       bigint;
  /** Sum of 10% maintenance margin across all cross positions. */
  maintMargin:     string;
  maintMarginWei:  bigint;
  /** equity − maintMargin (what can be freely withdrawn). */
  freeMargin:      string;
  freeMarginWei:   bigint;
  /** Cross liquidation threshold (= maintMargin). */
  liqThreshold:    string;
  /** Whether the cross account equity < maintenance margin. */
  liquidatable:    boolean;
  /** List of open position IDs in this cross account. */
  positionIds:     number[];
}

export interface OpenPositionParams {
  marketId:   number;
  isLong:     boolean;
  /** true = use cross-margin account; false = isolated position. */
  isCross:    boolean;
  /** Collateral to deposit (wei). For cross: deducted from cross balance. */
  collateral: bigint;
  /** Leverage multiplier (1 – market.maxLeverage). Size = (col-fee) × leverage. */
  leverage:   number;
  /** Stop-loss oracle price (0 = none). Long: must be < markPrice. Short: > markPrice. */
  slPrice:    bigint;
  /** Take-profit oracle price (0 = none). Long: must be > markPrice. Short: < markPrice. */
  tpPrice:    bigint;
}

/** Off-chain PnL estimate (no RPC call required). */
export interface PnlQuote {
  /** Notional size (wei). */
  sizeWei:       bigint;
  entryPrice:    bigint;
  currentPrice:  bigint;
  /** Signed PnL in wei. Negative = loss. */
  pnlWei:        bigint;
  /** Formatted PnL string (e.g. "+123.45" or "−50.00"). */
  pnl:           string;
  sign:          "+" | "−";
  /** PnL as a percentage of notional size (e.g. "5.42"). */
  pnlPct:        string;
  /** Estimated ROE: PnL / initial collateral × 100 (e.g. "54.2"). */
  roe:           string;
}

// ─── Function selector computation ───────────────────────────────────────────

/** Compute the 4-byte ABI selector from a canonical function signature. */
function sel(sig: string): string {
  const bytes = new TextEncoder().encode(sig);
  const hash  = keccak_256(bytes);
  return "0x" + Array.from(hash.slice(0, 4), b => b.toString(16).padStart(2, "0")).join("");
}

// Memoised selectors (computed once at module initialisation)
const SEL = (() => {
  const s = sel;
  return {
    // Views — market
    getMarket:              s("getMarket(uint256)"),
    markPrice:              s("markPrice(uint256)"),
    currentFundingRate:     s("currentFundingRate(uint256)"),
    marketCount:            s("marketCount()"),
    // Views — position
    positions:              s("positions(uint256)"),
    unrealisedPnl:          s("unrealisedPnl(uint256)"),
    healthBps:              s("healthBps(uint256)"),
    liquidationPrice:       s("liquidationPrice(uint256)"),
    isLiquidatable:         s("isLiquidatable(uint256)"),
    isSLTriggered:          s("isSLTriggered(uint256)"),
    isTPTriggered:          s("isTPTriggered(uint256)"),
    // Views — cross
    crossBalance:           s("crossBalance(address)"),
    crossPositionIds:       s("crossPositionIds(address)"),
    isCrossLiquidatable:    s("isCrossLiquidatable(address)"),
    crossEquity:            s("crossEquity(address)"),
    crossMaintMargin:       s("crossMaintMargin(address)"),
    freeCrossMargin:        s("freeCrossMargin(address)"),
    crossLiquidationThreshold: s("crossLiquidationThreshold(address)"),
    // Admin views
    nextPositionId:         s("nextPositionId()"),
    protocolFeeBalance:     s("protocolFeeBalance()"),
    collateralToken:        s("collateralToken()"),
    owner:                  s("owner()"),
    // Writes — position
    openPosition:           s("openPosition(uint256,bool,uint256,uint256,bool,uint256,uint256)"),
    closePosition:          s("closePosition(uint256)"),
    partialClose:           s("partialClose(uint256,uint256)"),
    addCollateral:          s("addCollateral(uint256,uint256)"),
    setStopLoss:            s("setStopLoss(uint256,uint256)"),
    setTakeProfit:          s("setTakeProfit(uint256,uint256)"),
    setTrailingStop:        s("setTrailingStop(uint256,uint256)"),
    updateTrailingStop:     s("updateTrailingStop(uint256)"),
    triggerOrder:           s("triggerOrder(uint256)"),
    triggerStopLoss:        s("triggerStopLoss(uint256)"),
    triggerTakeProfit:      s("triggerTakeProfit(uint256)"),
    liquidate:              s("liquidate(uint256)"),
    liquidateCross:         s("liquidateCross(address)"),
    // Writes — cross
    depositCross:           s("depositCross(uint256)"),
    withdrawCross:          s("withdrawCross(uint256)"),
    // Writes — funding
    updateFunding:          s("updateFunding(uint256)"),
  };
})();

// ─── Helper ──────────────────────────────────────────────────────────────────

export class PerpHelper {
  constructor(
    private readonly client: ZbxClient,
    readonly contractAddress: string = "0x000000000000000000000000005a425045525053",
  ) {}

  // ══════════════════════════════════════════════════════════════════════════
  //  MARKET VIEWS
  // ══════════════════════════════════════════════════════════════════════════

  /**
   * Get full market info for a market ID, including live mark price.
   *
   * @example
   * const m = await zbx.perp.getMarket(0); // BTC-USD
   * console.log(m.symbol, "—", m.markPrice, "USD");
   * console.log("Funding in:", m.nextFundingIn, "s");
   * console.log("OI imbalance:", m.oiImbalance);
   */
  async getMarket(marketId: number): Promise<MarketInfo> {
    const idPad = pad64(marketId);
    const [rawM, rawPrice, rawFund] = await Promise.all([
      this._call(SEL.getMarket + idPad),
      this._call(SEL.markPrice + idPad),
      this._call(SEL.currentFundingRate + idPad),
    ]);

    // getMarket returns:
    // slot 0: string offset (dynamic)
    // slot 1: oracle address
    // slot 2: active bool
    // slot 3: maxLeverage
    // slot 4: totalLongOI
    // slot 5: totalShortOI
    // slot 6: cumulativeFunding (int256)
    // slot 7: nextFundingIn
    // slot 8+: string data
    const b        = rawM.replace(/^0x/, "");
    // first 32 bytes = offset to string data (dynamic head)
    const oracle   = "0x" + b.slice(64 + 24, 64 + 64);
    const active   = h2b(b.slice(128, 192)) !== 0n;
    const maxLev   = Number(h2b(b.slice(192, 256)));
    const longWei  = h2b(b.slice(256, 320));
    const shortWei = h2b(b.slice(320, 384));
    const nextFund = Number(h2b(b.slice(448, 512)));
    const fundRaw  = h2s64(rawFund.replace(/^0x/, "").slice(0, 64));

    const markWei  = h2b(rawPrice.replace(/^0x/, "").slice(0, 64));

    const imbalanceWei = longWei >= shortWei ? longWei - shortWei : shortWei - longWei;
    const imbalanceSign = longWei >= shortWei ? "+" : "−";

    return {
      marketId,
      symbol:          decodeAbiString(b),
      oracle,
      active,
      maxLeverage:     maxLev,
      totalLongOI:     fmtWei(longWei),
      totalLongOIWei:  longWei,
      totalShortOI:    fmtWei(shortWei),
      totalShortOIWei: shortWei,
      oiImbalance:     imbalanceSign + fmtWei(imbalanceWei),
      fundingRate:     fundRaw,
      nextFundingIn:   nextFund,
      markPrice:       fmtWei(markWei, 2),
      markPriceWei:    markWei,
    };
  }

  /**
   * Get all markets (parallel fetch).
   *
   * @example
   * const markets = await zbx.perp.getMarkets();
   * markets.forEach(m => console.log(`${m.symbol}: ${m.markPrice} USD`));
   */
  async getMarkets(): Promise<MarketInfo[]> {
    const raw   = await this._call(SEL.marketCount);
    const count = Number(h2b(raw.replace(/^0x/, "").slice(0, 64)));
    if (count === 0) return [];
    return Promise.all(Array.from({ length: count }, (_, i) => this.getMarket(i)));
  }

  /**
   * Get current mark price for a market.
   *
   * @example
   * const price = await zbx.perp.getMarkPrice(0);
   * console.log("BTC:", price, "USD");
   */
  async getMarkPrice(marketId: number): Promise<string> {
    const raw = await this._call(SEL.markPrice + pad64(marketId));
    return fmtWei(h2b(raw.replace(/^0x/, "").slice(0, 64)), 2);
  }

  /**
   * Get live funding rate for a market (int256, basis-point scale).
   *
   * @example
   * const rate = await zbx.perp.getFundingRate(0);
   * console.log("BTC funding:", rate, "per 8h");
   */
  async getFundingRate(marketId: number): Promise<string> {
    const raw = await this._call(SEL.currentFundingRate + pad64(marketId));
    return h2s64(raw.replace(/^0x/, "").slice(0, 64));
  }

  // ══════════════════════════════════════════════════════════════════════════
  //  POSITION VIEWS
  // ══════════════════════════════════════════════════════════════════════════

  /**
   * Get full position state including live PnL, health, and trigger status.
   * Fires 6 parallel RPC calls for a complete snapshot.
   *
   * @example
   * const pos = await zbx.perp.getPosition(1);
   * console.log(`PnL: ${pos.unrealisedPnl}`);
   * console.log(`Health: ${pos.healthBps / 100}%`);
   * console.log(`Liq price: ${pos.liquidationPrice}`);
   * if (pos.isSLTriggered) console.log("⚠ SL ready to trigger!");
   */
  async getPosition(positionId: number): Promise<Position> {
    const idPad = pad64(positionId);
    const [rawPos, rawPnl, rawHealth, rawLiq, rawSL, rawTP, rawIsLiq] = await Promise.all([
      this._call(SEL.positions + idPad),
      this._call(SEL.unrealisedPnl + idPad),
      this._call(SEL.healthBps + idPad),
      this._call(SEL.liquidationPrice + idPad),
      this._call(SEL.isSLTriggered + idPad),
      this._call(SEL.isTPTriggered + idPad),
      this._call(SEL.isLiquidatable + idPad),
    ]);

    const b = rawPos.replace(/^0x/, "");
    // Position struct ABI layout (each field = 32 bytes):
    // 0:  trader (address, right-padded)
    // 1:  marketId
    // 2:  isLong
    // 3:  isCross
    // 4:  collateral
    // 5:  size
    // 6:  entryPrice
    // 7:  fundingEntryRate (int256)
    // 8:  stopLoss
    // 9:  takeProfit
    // 10: trailBps
    // 11: trailPeak
    // 12: closed
    // 13: initialMargin
    const trader           = "0x" + b.slice(24,  64);
    const marketId         = Number(h2b(b.slice(64,  128)));
    const isLong           = h2b(b.slice(128, 192)) !== 0n;
    const isCross          = h2b(b.slice(192, 256)) !== 0n;
    const collateralWei    = h2b(b.slice(256, 320));
    const sizeWei          = h2b(b.slice(320, 384));
    const entryPriceWei    = h2b(b.slice(384, 448));
    const fundingEntryRate = h2s64(b.slice(448, 512));
    const stopLossWei      = h2b(b.slice(512, 576));
    const takeProfitWei    = h2b(b.slice(576, 640));
    const trailBps         = Number(h2b(b.slice(640, 704)));
    const trailPeakWei     = h2b(b.slice(704, 768));
    const closed           = h2b(b.slice(768, 832)) !== 0n;
    const initialMarginWei = h2b(b.slice(832, 896));

    // Live fields
    const pnlRawHex        = rawPnl.replace(/^0x/, "").slice(0, 64);
    const { signed: pnlSigned, abs: pnlAbs, sign: pnlSign } = decodeInt256(pnlRawHex);
    const healthRaw        = Number(h2b(rawHealth.replace(/^0x/, "").slice(0, 64)));
    const liqPriceWei      = h2b(rawLiq.replace(/^0x/, "").slice(0, 64));
    const isSLTriggered    = h2b(rawSL.replace(/^0x/, "").slice(0, 64)) !== 0n;
    const isTPTriggered    = h2b(rawTP.replace(/^0x/, "").slice(0, 64)) !== 0n;
    const isLiquidatable   = h2b(rawIsLiq.replace(/^0x/, "").slice(0, 64)) !== 0n;

    return {
      positionId,
      trader,
      marketId,
      isLong,
      isCross,
      collateral:          fmtWei(collateralWei),
      collateralWei,
      size:                fmtWei(sizeWei),
      sizeWei,
      entryPrice:          fmtWei(entryPriceWei, 2),
      entryPriceWei,
      fundingEntryRate,
      stopLoss:            stopLossWei > 0n ? fmtWei(stopLossWei, 2) : "0",
      stopLossWei,
      takeProfit:          takeProfitWei > 0n ? fmtWei(takeProfitWei, 2) : "0",
      takeProfitWei,
      trailBps,
      trailPeak:           fmtWei(trailPeakWei, 2),
      trailPeakWei,
      closed,
      initialMargin:       fmtWei(initialMarginWei),
      initialMarginWei,
      unrealisedPnl:       pnlSign + fmtWei(pnlAbs),
      unrealisedPnlWei:    pnlSigned,
      unrealisedPnlSign:   pnlSign,
      healthBps:           healthRaw,
      liquidationPrice:    liqPriceWei > 0n ? fmtWei(liqPriceWei, 2) : "0",
      liquidationPriceWei: liqPriceWei,
      isSLTriggered,
      isTPTriggered,
      isLiquidatable,
    };
  }

  /**
   * Get just the health of a position (cheap single RPC).
   * Returns 0 if liquidatable; 10000 if healthy.
   *
   * @example
   * const h = await zbx.perp.healthBps(42);
   * if (h < 500) console.warn("Position dangerously close to liquidation!");
   */
  async healthBps(positionId: number): Promise<number> {
    const raw = await this._call(SEL.healthBps + pad64(positionId));
    return Number(h2b(raw.replace(/^0x/, "").slice(0, 64)));
  }

  /** Check if a position is currently liquidatable. */
  async isLiquidatable(positionId: number): Promise<boolean> {
    const raw = await this._call(SEL.isLiquidatable + pad64(positionId));
    return h2b(raw.replace(/^0x/, "").slice(0, 64)) !== 0n;
  }

  /** Check if the stop-loss level has been crossed by the mark price. */
  async isSLTriggered(positionId: number): Promise<boolean> {
    const raw = await this._call(SEL.isSLTriggered + pad64(positionId));
    return h2b(raw.replace(/^0x/, "").slice(0, 64)) !== 0n;
  }

  /** Check if the take-profit level has been crossed by the mark price. */
  async isTPTriggered(positionId: number): Promise<boolean> {
    const raw = await this._call(SEL.isTPTriggered + pad64(positionId));
    return h2b(raw.replace(/^0x/, "").slice(0, 64)) !== 0n;
  }

  /** Get exact liquidation price for an isolated position. */
  async liquidationPrice(positionId: number): Promise<string> {
    const raw = await this._call(SEL.liquidationPrice + pad64(positionId));
    const wei = h2b(raw.replace(/^0x/, "").slice(0, 64));
    return wei > 0n ? fmtWei(wei, 2) : "0";
  }

  // ══════════════════════════════════════════════════════════════════════════
  //  CROSS-MARGIN VIEWS
  // ══════════════════════════════════════════════════════════════════════════

  /**
   * Get full cross-margin account state for a trader.
   *
   * @example
   * const acc = await zbx.perp.getCrossAccount("0xYourAddress");
   * console.log("Equity:", acc.equity);
   * console.log("Free margin:", acc.freeMargin);
   * if (acc.liquidatable) console.warn("Cross account at risk!");
   */
  async getCrossAccount(trader: string): Promise<CrossAccountState> {
    const addrPad = trader.slice(2).padStart(64, "0");
    const [rawBal, rawEq, rawMaint, rawFree, rawLiq, rawLiqBool, rawIds] = await Promise.all([
      this._call(SEL.crossBalance + addrPad),
      this._call(SEL.crossEquity + addrPad),
      this._call(SEL.crossMaintMargin + addrPad),
      this._call(SEL.freeCrossMargin + addrPad),
      this._call(SEL.crossLiquidationThreshold + addrPad),
      this._call(SEL.isCrossLiquidatable + addrPad),
      this._call(SEL.crossPositionIds + addrPad),
    ]);

    const balWei    = h2b(rawBal.replace(/^0x/, "").slice(0, 64));
    const { signed: eqSigned, abs: eqAbs } = decodeInt256(rawEq.replace(/^0x/, "").slice(0, 64));
    const maintWei  = h2b(rawMaint.replace(/^0x/, "").slice(0, 64));
    const freeWei   = h2b(rawFree.replace(/^0x/, "").slice(0, 64));
    const liqThresh = h2b(rawLiq.replace(/^0x/, "").slice(0, 64));
    const liquidatable = h2b(rawLiqBool.replace(/^0x/, "").slice(0, 64)) !== 0n;

    // Decode uint256[] from ABI
    const posIds    = decodeUint256Array(rawIds.replace(/^0x/, ""));

    return {
      trader,
      balance:         fmtWei(balWei),
      balanceWei:      balWei,
      equity:          (eqSigned < 0n ? "−" : "+") + fmtWei(eqAbs),
      equityWei:       eqSigned,
      maintMargin:     fmtWei(maintWei),
      maintMarginWei:  maintWei,
      freeMargin:      fmtWei(freeWei),
      freeMarginWei:   freeWei,
      liqThreshold:    fmtWei(liqThresh),
      liquidatable,
      positionIds:     posIds,
    };
  }

  /** Get the free cross-margin available for withdrawal or new positions. */
  async freeCrossMargin(trader: string): Promise<bigint> {
    const raw = await this._call(SEL.freeCrossMargin + trader.slice(2).padStart(64, "0"));
    return h2b(raw.replace(/^0x/, "").slice(0, 64));
  }

  /** Check whether a cross account is currently liquidatable. */
  async isCrossLiquidatable(trader: string): Promise<boolean> {
    const raw = await this._call(SEL.isCrossLiquidatable + trader.slice(2).padStart(64, "0"));
    return h2b(raw.replace(/^0x/, "").slice(0, 64)) !== 0n;
  }

  // ══════════════════════════════════════════════════════════════════════════
  //  OFF-CHAIN COMPUTATION HELPERS (zero RPC calls)
  // ══════════════════════════════════════════════════════════════════════════

  /**
   * Estimate PnL for a position given a target price — no RPC required.
   *
   * @example
   * // My long: 10 ZBX notional, entry 50000, current 52000
   * const q = zbx.perp.quotePnL({
   *   isLong:       true,
   *   entryPrice:   zbx.parseZbx("50000"),
   *   currentPrice: zbx.parseZbx("52000"),
   *   size:         zbx.parseZbx("10"),
   *   collateral:   zbx.parseZbx("1"),
   * });
   * console.log("PnL:", q.pnl, "ROE:", q.roe, "%");
   */
  quotePnL(params: {
    isLong:       boolean;
    entryPrice:   bigint;
    currentPrice: bigint;
    size:         bigint;
    collateral:   bigint;
  }): PnlQuote {
    const { isLong, entryPrice, currentPrice, size, collateral } = params;
    if (entryPrice === 0n) throw new Error("PerpHelper.quotePnL: entryPrice cannot be 0");

    // pnl = (currentPrice - entryPrice) / entryPrice * size  [long]
    //     = (entryPrice - currentPrice) / entryPrice * size  [short]
    let pnlWei: bigint;
    if (isLong) {
      pnlWei = (BigInt(currentPrice) - BigInt(entryPrice)) * BigInt(size) / BigInt(entryPrice);
    } else {
      pnlWei = (BigInt(entryPrice) - BigInt(currentPrice)) * BigInt(size) / BigInt(entryPrice);
    }

    const sign: "+" | "−" = pnlWei >= 0n ? "+" : "−";
    const abs = pnlWei < 0n ? -pnlWei : pnlWei;

    const pnlPct = size > 0n
      ? (Number(pnlWei < 0n ? -pnlWei : pnlWei) / Number(size) * 100).toFixed(2)
      : "0.00";
    const roe = collateral > 0n
      ? (Number(pnlWei < 0n ? -pnlWei : pnlWei) / Number(collateral) * 100 * (pnlWei >= 0n ? 1 : -1)).toFixed(2)
      : "0.00";

    return {
      sizeWei:      size,
      entryPrice,
      currentPrice,
      pnlWei,
      pnl:          sign + fmtWei(abs),
      sign,
      pnlPct,
      roe,
    };
  }

  /**
   * Estimate liquidation price for an isolated position — no RPC required.
   * Matches the contract's `liquidationPrice()` formula exactly.
   *
   * Formula:
   *   LONG:  liqPrice = entry + entry × (MM − col) / size
   *   SHORT: liqPrice = entry − entry × (MM − col) / size
   *   where MM = size × MAINTENANCE_MARGIN_BPS / 10000
   *
   * @example
   * const liq = zbx.perp.calcLiquidationPrice({
   *   isLong:     true,
   *   entryPrice: zbx.parseZbx("50000"),
   *   size:       zbx.parseZbx("10"),
   *   collateral: zbx.parseZbx("1"),
   * });
   * console.log("Liq at:", liq, "USD");
   */
  calcLiquidationPrice(params: {
    isLong:     boolean;
    entryPrice: bigint;
    size:       bigint;
    collateral: bigint;
  }): string {
    const { isLong, entryPrice, size, collateral } = params;
    if (size === 0n) throw new Error("PerpHelper.calcLiquidationPrice: size cannot be 0");

    const MM        = (size * PERP_CONSTANTS.MAINTENANCE_MARGIN_BPS) / 10_000n;
    const numerator = MM >= collateral ? MM - collateral : -(collateral - MM);
    const delta     = (entryPrice * (numerator < 0n ? -numerator : numerator)) / size;

    let liqPrice: bigint;
    if (isLong) {
      liqPrice = numerator >= 0n ? entryPrice + delta : entryPrice - delta;
    } else {
      liqPrice = numerator >= 0n ? entryPrice - delta : entryPrice + delta;
    }

    return liqPrice > 0n ? fmtWei(liqPrice, 2) : "0";
  }

  /**
   * Validate `openPosition` parameters before sending the transaction.
   * Returns null on success, or an error message string on failure.
   *
   * @example
   * const err = await zbx.perp.validateOpen({ marketId: 0, isLong: true, collateral: parseZbx("100"), leverage: 10, isCross: false, slPrice: 0n, tpPrice: 0n });
   * if (err) throw new Error(err);
   */
  async validateOpen(params: OpenPositionParams): Promise<string | null> {
    if (params.collateral === 0n) return "collateral must be > 0";
    if (params.leverage < 1)     return "leverage must be >= 1";

    const market = await this.getMarket(params.marketId);
    if (!market.active) return `market ${params.marketId} is inactive`;
    if (params.leverage > market.maxLeverage) {
      return `leverage ${params.leverage} exceeds market max (${market.maxLeverage})`;
    }

    const markWei = market.markPriceWei;

    if (params.slPrice > 0n) {
      if (params.isLong  && params.slPrice >= markWei) return "long SL must be < mark price";
      if (!params.isLong && params.slPrice <= markWei) return "short SL must be > mark price";
    }
    if (params.tpPrice > 0n) {
      if (params.isLong  && params.tpPrice <= markWei) return "long TP must be > mark price";
      if (!params.isLong && params.tpPrice >= markWei) return "short TP must be < mark price";
    }

    return null;
  }

  /**
   * Scan a list of position IDs and return those ready to liquidate/trigger.
   * Useful for keeper bots.
   *
   * @example
   * const toTrigger = await zbx.perp.scanPositions([1, 2, 3, 4, 5]);
   * toTrigger.forEach(({ positionId, reason }) =>
   *   console.log(`Position ${positionId}: ${reason}`));
   */
  async scanPositions(positionIds: number[]): Promise<Array<{
    positionId: number;
    reason: "liquidatable" | "sl_triggered" | "tp_triggered";
  }>> {
    const results = await Promise.all(positionIds.map(async id => {
      const [liq, sl, tp] = await Promise.all([
        this.isLiquidatable(id),
        this.isSLTriggered(id),
        this.isTPTriggered(id),
      ]);
      return { positionId: id, liq, sl, tp };
    }));

    const triggerable: Array<{ positionId: number; reason: "liquidatable" | "sl_triggered" | "tp_triggered" }> = [];
    for (const r of results) {
      if (r.liq) triggerable.push({ positionId: r.positionId, reason: "liquidatable" });
      else if (r.sl) triggerable.push({ positionId: r.positionId, reason: "sl_triggered" });
      else if (r.tp) triggerable.push({ positionId: r.positionId, reason: "tp_triggered" });
    }
    return triggerable;
  }

  // ══════════════════════════════════════════════════════════════════════════
  //  TRANSACTION ENCODERS — Position
  // ══════════════════════════════════════════════════════════════════════════

  /**
   * Encode `openPosition(uint256,bool,uint256,uint256,bool,uint256,uint256)`.
   *
   * Contract signature: openPosition(marketId, isLong, collateral, leverage, isCross, slPrice, tpPrice)
   * Size is computed on-chain: size = (collateral - fee) × leverage
   *
   * IMPORTANT: for isolated positions, approve the collateralToken for this amount before calling.
   * For cross positions, ensure the cross account has enough balance.
   *
   * @example
   * const data = zbx.perp.encodeOpenPosition({
   *   marketId:   0,
   *   isLong:     true,
   *   isCross:    false,
   *   collateral: zbx.parseZbx("100"),
   *   leverage:   10,
   *   slPrice:    0n,
   *   tpPrice:    0n,
   * });
   */
  encodeOpenPosition(p: OpenPositionParams): string {
    const b = (v: bigint | number | boolean): string =>
      (typeof v === "boolean" ? (v ? 1n : 0n) : BigInt(v)).toString(16).padStart(64, "0");
    return SEL.openPosition
      + b(p.marketId)
      + b(p.isLong)
      + b(p.collateral)
      + b(p.leverage)
      + b(p.isCross)
      + b(p.slPrice)
      + b(p.tpPrice);
  }

  /**
   * Encode `closePosition(uint256)` — fully close an isolated or cross position.
   */
  encodeClosePosition(positionId: number): string {
    return SEL.closePosition + pad64(positionId);
  }

  /**
   * Encode `partialClose(uint256 positionId, uint256 closeBps)`.
   * `closeBps` is in basis points (1–10000). 5000 = 50%, 10000 = 100% (full close).
   *
   * @example
   * // Close 30% of position 1
   * const data = zbx.perp.encodePartialClose(1, 3000);
   */
  encodePartialClose(positionId: number, closeBps: number): string {
    if (closeBps <= 0 || closeBps > 10_000) throw new Error("closeBps must be 1–10000");
    return SEL.partialClose + pad64(positionId) + pad64(closeBps);
  }

  /**
   * Encode `addCollateral(uint256 positionId, uint256 amount)` (isolated only).
   * Increases margin → moves liquidation price further away from entry.
   * Approve collateralToken for `amount` before calling.
   */
  encodeAddCollateral(positionId: number, amountWei: bigint): string {
    return SEL.addCollateral + pad64(positionId) + amountWei.toString(16).padStart(64, "0");
  }

  // ══════════════════════════════════════════════════════════════════════════
  //  TRANSACTION ENCODERS — SL / TP / Trailing Stop
  // ══════════════════════════════════════════════════════════════════════════

  /**
   * Encode `setStopLoss(uint256 positionId, uint256 slPrice)`.
   * Update or remove (slPrice=0) stop-loss after position is open.
   * SL price must satisfy: long → slPrice < markPrice; short → slPrice > markPrice.
   */
  encodeSetStopLoss(positionId: number, slPrice: bigint): string {
    return SEL.setStopLoss + pad64(positionId) + slPrice.toString(16).padStart(64, "0");
  }

  /**
   * Encode `setTakeProfit(uint256 positionId, uint256 tpPrice)`.
   * Update or remove (tpPrice=0) take-profit after position is open.
   * TP price must satisfy: long → tpPrice > markPrice; short → tpPrice < markPrice.
   */
  encodeSetTakeProfit(positionId: number, tpPrice: bigint): string {
    return SEL.setTakeProfit + pad64(positionId) + tpPrice.toString(16).padStart(64, "0");
  }

  /**
   * Encode `setTrailingStop(uint256 positionId, uint256 trailBps)`.
   * `trailBps` = trail width in basis points (1–5000, i.e. 0.01%–50%).
   * Sets a trailing stop that automatically ratchets as mark price improves.
   * Overwrites any existing static stop-loss.
   *
   * @example
   * // 2% trailing stop on position 1
   * const data = zbx.perp.encodeSetTrailingStop(1, 200);
   */
  encodeSetTrailingStop(positionId: number, trailBps: number): string {
    if (trailBps <= 0 || trailBps > 5000) throw new Error("trailBps must be 1–5000");
    return SEL.setTrailingStop + pad64(positionId) + pad64(trailBps);
  }

  /**
   * Encode `updateTrailingStop(uint256 positionId)` — keeper ratchets the trailing SL.
   * Call this when mark price has moved favourably past the last peak.
   * Reverts with `TrailNotFavourable` if price has not improved.
   */
  encodeUpdateTrailingStop(positionId: number): string {
    return SEL.updateTrailingStop + pad64(positionId);
  }

  // ══════════════════════════════════════════════════════════════════════════
  //  TRANSACTION ENCODERS — Keeper Triggers
  // ══════════════════════════════════════════════════════════════════════════

  /**
   * Encode `triggerOrder(uint256 positionId)`.
   * Unified trigger — executes whichever of SL or TP is currently hit.
   * Earns 0.05% KEEPER_BOUNTY. Reverts with `NeitherTriggered` if neither is active.
   *
   * @example
   * // Keeper bot: trigger SL or TP for position 42
   * const data = zbx.perp.encodeTriggerOrder(42);
   */
  encodeTriggerOrder(positionId: number): string {
    return SEL.triggerOrder + pad64(positionId);
  }

  /**
   * Encode `triggerStopLoss(uint256 positionId)` — execute SL specifically.
   * Reverts with `SLNotTriggered` if mark price has not crossed SL.
   */
  encodeTriggerStopLoss(positionId: number): string {
    return SEL.triggerStopLoss + pad64(positionId);
  }

  /**
   * Encode `triggerTakeProfit(uint256 positionId)` — execute TP specifically.
   * Reverts with `TPNotTriggered` if mark price has not crossed TP.
   */
  encodeTriggerTakeProfit(positionId: number): string {
    return SEL.triggerTakeProfit + pad64(positionId);
  }

  // ══════════════════════════════════════════════════════════════════════════
  //  TRANSACTION ENCODERS — Liquidation
  // ══════════════════════════════════════════════════════════════════════════

  /**
   * Encode `liquidate(uint256 positionId)` — liquidate an isolated position.
   * Earns 1% LIQUIDATION_BOUNTY of the position's collateral.
   * Reverts with `NotLiquidatable` if health is still above threshold.
   *
   * @example
   * const data = zbx.perp.encodeLiquidate(42);
   */
  encodeLiquidate(positionId: number): string {
    return SEL.liquidate + pad64(positionId);
  }

  /**
   * Encode `liquidateCross(address trader)` — liquidate an entire cross account.
   * Earns 1% LIQUIDATION_BOUNTY of the cross account balance.
   * Reverts with `NotLiquidatable` if cross equity > maint margin.
   *
   * @example
   * const data = zbx.perp.encodeLiquidateCross("0xTraderAddress");
   */
  encodeLiquidateCross(trader: string): string {
    return SEL.liquidateCross + trader.slice(2).padStart(64, "0");
  }

  // ══════════════════════════════════════════════════════════════════════════
  //  TRANSACTION ENCODERS — Cross Margin
  // ══════════════════════════════════════════════════════════════════════════

  /**
   * Encode `depositCross(uint256 amount)` — add collateral to cross account.
   * Approve the collateralToken first.
   */
  encodeDepositCross(amountWei: bigint): string {
    return SEL.depositCross + amountWei.toString(16).padStart(64, "0");
  }

  /**
   * Encode `withdrawCross(uint256 amount)` — withdraw free margin from cross account.
   * Reverts with `CrossWithdrawTooLarge` if amount > freeCrossMargin.
   */
  encodeWithdrawCross(amountWei: bigint): string {
    return SEL.withdrawCross + amountWei.toString(16).padStart(64, "0");
  }

  // ══════════════════════════════════════════════════════════════════════════
  //  TRANSACTION ENCODERS — Funding
  // ══════════════════════════════════════════════════════════════════════════

  /**
   * Encode `updateFunding(uint256 marketId)` — manually settle an overdue funding cycle.
   * Anyone can call this. The update is also triggered automatically by any trade.
   */
  encodeUpdateFunding(marketId: number): string {
    return SEL.updateFunding + pad64(marketId);
  }

  // ══════════════════════════════════════════════════════════════════════════
  //  PRIVATE
  // ══════════════════════════════════════════════════════════════════════════

  private async _call(data: string): Promise<string> {
    return this.client.rpc<string>("eth_call", [
      { to: this.contractAddress, data },
      "latest",
    ]);
  }
}

// ── Utilities ─────────────────────────────────────────────────────────────────

function pad64(n: number | bigint): string {
  return BigInt(n).toString(16).padStart(64, "0");
}

function h2b(hex: string): bigint {
  return hex ? BigInt("0x" + hex) : 0n;
}

/** Decode a signed int256 from a 64-char (256-bit) hex string. */
function decodeInt256(hex64: string): { signed: bigint; abs: bigint; sign: "+" | "−" } {
  const n      = BigInt("0x" + hex64);
  const MAX_I  = 2n ** 255n;
  const signed = n >= MAX_I ? n - 2n ** 256n : n;
  const abs    = signed < 0n ? -signed : signed;
  const sign: "+" | "−" = signed < 0n ? "−" : "+";
  return { signed, abs, sign };
}

/** Decode a signed int256 from a 64-char hex string as a formatted string. */
function h2s64(hex64: string): string {
  const { sign, abs } = decodeInt256(hex64);
  return sign + fmtWei(abs, 4);
}

/** Decode ABI-encoded string (dynamic, head = offset at position 0). */
function decodeAbiString(abiEncoded: string): string {
  try {
    // Find the string offset (first 32-byte slot)
    const offsetHex = abiEncoded.slice(0, 64);
    const offset    = Number(h2b(offsetHex)); // byte offset from start of data
    const slotStart = offset * 2;            // hex position
    const lenHex    = abiEncoded.slice(slotStart, slotStart + 64);
    const len       = Number(h2b(lenHex));
    if (len === 0) return "";
    const strHex    = abiEncoded.slice(slotStart + 64, slotStart + 64 + len * 2);
    const bytes     = new Uint8Array(len);
    for (let i = 0; i < len; i++) {
      bytes[i] = parseInt(strHex.slice(i * 2, i * 2 + 2), 16);
    }
    return new TextDecoder("utf-8").decode(bytes);
  } catch {
    return "UNKNOWN";
  }
}

/** Decode ABI-encoded uint256[] (dynamic array). */
function decodeUint256Array(hex: string): number[] {
  if (!hex || hex.length < 128) return [];
  try {
    // offset to array data (first slot)
    const offset  = Number(h2b(hex.slice(0, 64)));
    const lenStart = offset * 2;
    const len     = Number(h2b(hex.slice(lenStart, lenStart + 64)));
    const result: number[] = [];
    for (let i = 0; i < len; i++) {
      const start = lenStart + 64 + i * 64;
      result.push(Number(h2b(hex.slice(start, start + 64))));
    }
    return result;
  } catch {
    return [];
  }
}

function fmtWei(wei: bigint, dec = 4): string {
  const whole   = wei / 10n ** 18n;
  const frac    = wei % 10n ** 18n;
  const fracStr = frac.toString().padStart(18, "0").slice(0, dec).replace(/0+$/, "");
  return fracStr ? `${whole}.${fracStr}` : whole.toString();
}
