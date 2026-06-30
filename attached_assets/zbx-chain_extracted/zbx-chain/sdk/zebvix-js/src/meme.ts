/**
 * MemeHelper — ZbxMemeFactory pump.fun-style bonding curve interaction.
 * Accessible via `client.meme.*`
 *
 * Anyone can launch a meme coin in one tx. No presale. No VC. Full fair launch.
 * Constant-product bonding curve → auto-graduates to ZbxAMM when threshold reached.
 *
 * Contract: ZbxMemeFactory.sol — ZEP-045
 *
 * @example
 * import { ZbxClient } from "zebvix.js";
 *
 * const zbx    = new ZbxClient("https://rpc.zebvix.io");
 * const wallet = zbx.wallet(process.env.PRIVATE_KEY!);
 *
 * // Launch a meme coin (costs 0.01 ZBX launch fee)
 * const launchData = zbx.meme.encodeLaunch({
 *   name:     "Doge ZBX",
 *   symbol:   "DOGZBX",
 *   imageUri: "ipfs://QmXxx",
 * });
 * const hash = await wallet.sendTx({
 *   to: zbx.meme.contractAddress, data: launchData,
 *   value: zbx.parseZbx("0.01"),
 * });
 *
 * // Quote buying 1 ZBX worth of meme tokens
 * const quote = await zbx.meme.quoteBuy(0, "1");
 * console.log("Get:", quote.tokensOut, "tokens for 1 ZBX");
 * console.log("Price impact:", quote.priceImpact, "%");
 *
 * // Buy with 1% slippage tolerance
 * const minOut = (BigInt(quote.tokensOutWei) * 99n) / 100n;
 * const buyData = zbx.meme.encodeBuy(0, minOut);
 * await wallet.sendTx({ to: zbx.meme.contractAddress, data: buyData, value: zbx.parseZbx("1") });
 */
import type { ZbxClient } from "./client";

// ─── Types ───────────────────────────────────────────────────────────────────

export interface MemeInfo {
  memeId:         number;
  token:          string;
  creator:        string;
  name:           string;
  symbol:         string;
  imageUri:       string;
  /** ZBX raised so far on bonding curve (formatted). */
  zbxRaised:      string;
  zbxRaisedWei:   bigint;
  /** ZBX needed to graduate (formatted). */
  graduationTarget: string;
  /** Percentage towards graduation (0–100). */
  progressPercent: string;
  /** Virtual ZBX reserve (constant product). */
  virtualZbx:     string;
  /** Virtual token reserve. */
  virtualToken:   string;
  /** Current spot price (ZBX per token, 18-decimal). */
  spotPrice:      string;
  graduated:      boolean;
  active:         boolean;
  totalBuys:      number;
  totalSells:     number;
  totalVolume:    string;
  launchBlock:    number;
}

export interface MemeBuyQuote {
  zbxIn:         string;
  /** Tokens out (formatted). */
  tokensOut:     string;
  tokensOutWei:  bigint;
  priceImpact:   string;
  /** Fee deducted (formatted ZBX). */
  fee:           string;
}

export interface MemeSellQuote {
  tokensIn:   string;
  /** ZBX out after fee (formatted). */
  zbxOut:     string;
  zbxOutWei:  bigint;
  priceImpact: string;
  fee:        string;
}

// ─── ABI selectors ───────────────────────────────────────────────────────────

const SEL = {
  launchMeme:  "0x7f4e0a4c", // launchMeme(string name, string symbol, string imageUri)
  buy:         "0xd96a094a", // buy(uint256 memeId, uint256 minOut) payable
  sell:        "0x57f68601", // sell(uint256 memeId, uint256 tokenAmount, uint256 minZbxOut)
  memes:       "0x69fee0c6", // memes(uint256) → Meme struct
  memeCount:   "0x8ada066e", // memeCount()
  tokenToMemeId:"0x8e7d9b79", // tokenToMemeId(address) → uint256
} as const;

// Constants matching ZbxMemeFactory.sol
const VIRTUAL_ZBX      = 30n * 10n ** 18n;   // 30 ZBX virtual seed
const TOTAL_SUPPLY     = 1_000_000_000n * 10n ** 18n; // 1B tokens
const GRADUATION_ZBX   = 30n * 10n ** 18n;   // 30 ZBX threshold
const TRADE_FEE_BPS    = 100n; // 1%

// ─── Helper ──────────────────────────────────────────────────────────────────

export class MemeHelper {
  constructor(
    private readonly client: ZbxClient,
    readonly contractAddress: string = "0x000000000000000000000000005a424d454d4521",
  ) {}

  // ── Views ──────────────────────────────────────────────────────────────────

  /**
   * Get info about a specific meme coin.
   *
   * @example
   * const meme = await zbx.meme.getMeme(0);
   * console.log(meme.symbol, "— progress:", meme.progressPercent, "%");
   */
  async getMeme(memeId: number): Promise<MemeInfo> {
    const raw = await this._call(SEL.memes + memeId.toString(16).padStart(64, "0"));
    const b   = raw.replace(/^0x/, "");

    const token     = "0x" + b.slice(24, 64);
    const creator   = "0x" + b.slice(88, 128);
    const zbxRaisedWei  = h2b(b.slice(128, 192));
    const virtualZbxWei = h2b(b.slice(192, 256));
    const virtualTokWei = h2b(b.slice(256, 320));
    const graduated     = h2b(b.slice(320, 384)) !== 0n;
    const active        = h2b(b.slice(384, 448)) !== 0n;
    const totalBuys     = Number(h2b(b.slice(448, 512)));
    const totalSells    = Number(h2b(b.slice(512, 576)));
    const totalVolumeWei= h2b(b.slice(576, 640));
    const launchBlock   = Number(h2b(b.slice(640, 704)));

    const progress = GRADUATION_ZBX > 0n
      ? Math.min(100, Number((zbxRaisedWei * 100n) / GRADUATION_ZBX))
      : 0;

    const spotPrice = virtualTokWei > 0n
      ? fmtWei((virtualZbxWei * 10n ** 18n) / virtualTokWei, 8)
      : "0";

    return {
      memeId,
      token,
      creator,
      name:             "", // decoded from on-chain string (event-indexed only)
      symbol:           "", // decoded from token contract
      imageUri:         "",
      zbxRaised:        fmtWei(zbxRaisedWei),
      zbxRaisedWei,
      graduationTarget: fmtWei(GRADUATION_ZBX),
      progressPercent:  progress.toFixed(1),
      virtualZbx:       fmtWei(virtualZbxWei),
      virtualToken:     fmtWei(virtualTokWei),
      spotPrice,
      graduated,
      active,
      totalBuys,
      totalSells,
      totalVolume:      fmtWei(totalVolumeWei),
      launchBlock,
    };
  }

  /**
   * Get total number of meme coins launched.
   *
   * @example
   * const count = await zbx.meme.getMemeCount();
   * console.log(count, "meme coins live");
   */
  async getMemeCount(): Promise<number> {
    const raw = await this._call(SEL.memeCount);
    return Number(h2b(raw.replace(/^0x/, "").slice(0, 64)));
  }

  /**
   * Quote buying meme tokens with ZBX.
   * Uses constant-product formula matching the on-chain bonding curve.
   *
   * @example
   * const q = await zbx.meme.quoteBuy(0, "1");  // spend 1 ZBX
   * console.log("Get:", q.tokensOut, "tokens");
   * console.log("Impact:", q.priceImpact, "%");
   */
  async quoteBuy(memeId: number, zbxAmountStr: string): Promise<MemeBuyQuote> {
    const meme  = await this.getMeme(memeId);
    const zbxIn = pWei(zbxAmountStr);

    const fee      = (zbxIn * TRADE_FEE_BPS) / 10_000n;
    const zbxNet   = zbxIn - fee;

    const vZbx = pWei(meme.virtualZbx);
    const vTok = pWei(meme.virtualToken);

    if (vZbx === 0n || vTok === 0n) throw new Error("MemeHelper: pool not initialised");

    // tokensOut = virtualToken * zbxNet / (virtualZbx + zbxNet)
    const tokensOut = (vTok * zbxNet) / (vZbx + zbxNet);

    const spotBefore = Number(vZbx) / Number(vTok);
    const spotAfter  = Number(vZbx + zbxNet) / Number(vTok - tokensOut);
    const impact     = Math.abs((spotAfter - spotBefore) / spotBefore * 100).toFixed(2);

    return {
      zbxIn:       zbxAmountStr,
      tokensOut:   fmtWei(tokensOut),
      tokensOutWei: tokensOut,
      priceImpact: impact,
      fee:         fmtWei(fee),
    };
  }

  /**
   * Quote selling meme tokens for ZBX.
   *
   * @example
   * const q = await zbx.meme.quoteSell(0, "1000000");  // sell 1M tokens
   * console.log("Get:", q.zbxOut, "ZBX");
   */
  async quoteSell(memeId: number, tokenAmountStr: string): Promise<MemeSellQuote> {
    const meme  = await this.getMeme(memeId);
    const tokIn = pWei(tokenAmountStr);

    const vZbx = pWei(meme.virtualZbx);
    const vTok = pWei(meme.virtualToken);

    if (vZbx === 0n || vTok === 0n) throw new Error("MemeHelper: pool not initialised");

    // zbxOut = virtualZbx * tokenAmount / (virtualToken + tokenAmount)
    const zbxOut = (vZbx * tokIn) / (vTok + tokIn);
    const fee    = (zbxOut * TRADE_FEE_BPS) / 10_000n;
    const netOut = zbxOut - fee;

    const spotBefore = Number(vZbx) / Number(vTok);
    const spotAfter  = Number(vZbx - zbxOut) / Number(vTok + tokIn);
    const impact     = Math.abs((spotAfter - spotBefore) / spotBefore * 100).toFixed(2);

    return {
      tokensIn:   tokenAmountStr,
      zbxOut:     fmtWei(netOut),
      zbxOutWei:  netOut,
      priceImpact: impact,
      fee:        fmtWei(fee),
    };
  }

  // ── Transaction encoders ───────────────────────────────────────────────────

  /**
   * Encode `launchMeme(string, string, string)` calldata.
   * Send `value = 0.01 ZBX` (LAUNCH_FEE) along with this call.
   *
   * @example
   * const data = zbx.meme.encodeLaunch({ name: "DogZBX", symbol: "DOGZBX", imageUri: "ipfs://..." });
   * await wallet.sendTx({ to: zbx.meme.contractAddress, data, value: zbx.parseZbx("0.01") });
   */
  encodeLaunch(params: { name: string; symbol: string; imageUri: string }): string {
    const encStr = (s: string): string => {
      const bytes = new TextEncoder().encode(s);
      const offset   = "0000000000000000000000000000000000000000000000000000000000000060"; // placeholder
      const lenHex   = bytes.length.toString(16).padStart(64, "0");
      const dataHex  = Array.from(bytes, b => b.toString(16).padStart(2, "0")).join("")
                         .padEnd(Math.ceil(bytes.length / 32) * 64, "0");
      return lenHex + dataHex;
    };

    // Dynamic string ABI encoding: offsets then data
    const nameBytes   = new TextEncoder().encode(params.name);
    const symbolBytes = new TextEncoder().encode(params.symbol);
    const imageBytes  = new TextEncoder().encode(params.imageUri);

    const offset1 = (96).toString(16).padStart(64, "0");
    const offset2 = (96 + 32 + Math.ceil(nameBytes.length / 32) * 32).toString(16).padStart(64, "0");
    const offset3 = (
      96 + 32 + Math.ceil(nameBytes.length / 32) * 32
         + 32 + Math.ceil(symbolBytes.length / 32) * 32
    ).toString(16).padStart(64, "0");

    const encStr2 = (b: Uint8Array): string =>
      b.length.toString(16).padStart(64, "0")
      + Array.from(b, x => x.toString(16).padStart(2, "0")).join("").padEnd(Math.ceil(b.length / 32) * 64, "0");

    return SEL.launchMeme
      + offset1
      + offset2
      + offset3
      + encStr2(nameBytes)
      + encStr2(symbolBytes)
      + encStr2(imageBytes);
  }

  /**
   * Encode `buy(uint256 memeId, uint256 minOut)` calldata.
   * Send the ZBX amount as transaction value.
   *
   * @example
   * const quote   = await zbx.meme.quoteBuy(0, "1");
   * const minOut  = (quote.tokensOutWei * 99n) / 100n; // 1% slippage
   * const data    = zbx.meme.encodeBuy(0, minOut);
   * await wallet.sendTx({ to: zbx.meme.contractAddress, data, value: zbx.parseZbx("1") });
   */
  encodeBuy(memeId: number, minOutWei: bigint): string {
    return SEL.buy
      + memeId.toString(16).padStart(64, "0")
      + minOutWei.toString(16).padStart(64, "0");
  }

  /**
   * Encode `sell(uint256 memeId, uint256 tokenAmount, uint256 minZbxOut)` calldata.
   *
   * @example
   * const quote   = await zbx.meme.quoteSell(0, "1000000");
   * const minZbx  = (quote.zbxOutWei * 99n) / 100n;
   * const data    = zbx.meme.encodeSell(0, zbx.parseZbx("1000000"), minZbx);
   */
  encodeSell(memeId: number, tokenAmountWei: bigint, minZbxOutWei: bigint): string {
    return SEL.sell
      + memeId.toString(16).padStart(64, "0")
      + tokenAmountWei.toString(16).padStart(64, "0")
      + minZbxOutWei.toString(16).padStart(64, "0");
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
