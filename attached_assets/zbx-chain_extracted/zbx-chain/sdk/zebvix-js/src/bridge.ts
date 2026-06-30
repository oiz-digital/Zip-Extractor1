/**
 * BridgeHelper — ZbxBridge cross-chain token transfers.
 * Accessible via `client.bridge.*`
 *
 * Enables moving ERC-20 tokens between ZBX Chain and other EVM networks.
 * Outbound: lock on source chain → mint on destination.
 * Inbound:  multi-relayer threshold consensus → release on ZBX Chain.
 *
 * Contract: ZbxBridge.sol — ZEP-003
 *
 * @example
 * import { ZbxClient } from "zebvix.js";
 *
 * const zbx    = new ZbxClient("https://rpc.zebvix.io");
 * const wallet = zbx.wallet(process.env.PRIVATE_KEY!);
 *
 * // Bridge 100 USDC from ZBX Chain to Ethereum
 * const data = zbx.bridge.encodeBridgeOut(
 *   "0xUSDC_address",
 *   "100",
 *   "0xYourEthAddress",
 * );
 * const hash = await wallet.sendTx({ to: zbx.bridge.contractAddress, data });
 *
 * // Check bridge limits for a token
 * const limit = await zbx.bridge.getHourlyLimit(8989, "0xUSDC_address");
 * console.log("Hourly bridge limit:", limit.limitZbx, "tokens");
 */
import type { ZbxClient } from "./client";

// ─── Types ───────────────────────────────────────────────────────────────────

export interface BridgeTokenInfo {
  /** Whether the token is whitelisted for bridging. */
  whitelisted:    boolean;
  /** Maximum amount per bridge transaction (wei). */
  maxAmountWei:   bigint;
  /** Maximum amount formatted. */
  maxAmount:      string;
  /** Total amount locked in bridge for this token (wei). */
  lockedWei:      bigint;
  locked:         string;
}

export interface BridgeHourlyWindow {
  /** Hourly bridge-in cap for (srcChainId, token) pair (wei). */
  limitWei:   bigint;
  limit:      string;
  /** Volume used in the current hourly window (wei). */
  usedWei:    bigint;
  used:       string;
  /** Volume remaining in the current hourly window (wei). */
  remainingWei: bigint;
  remaining:  string;
}

// ─── ABI selectors ───────────────────────────────────────────────────────────

const SEL = {
  bridgeOut:        "0x3ae2a594", // bridgeOut(address token, uint256 amount, bytes targetAddress)
  whitelistedTokens:"0x31d98b3f", // whitelistedTokens(address) → (bool, uint256)
  lockedAmount:     "0xd7b0b72f", // lockedAmount(address)
  bridgeInHourlyLimit:"0x4c6d8bca", // bridgeInHourlyLimit(uint256 srcChainId, address token)
  bridgeInVolume:   "0x7ae1cfca", // bridgeInVolume(uint256 srcChainId, address token)
  threshold:        "0x42cde4e8", // threshold()
  relayerCount:     "0xd2d418b9", // relayerCount()
  processedNonces:  "0x6e6d37b4", // processedNonces(bytes32)
  paused:           "0x5c975abb", // paused()
} as const;

// ─── Helper ──────────────────────────────────────────────────────────────────

export class BridgeHelper {
  constructor(
    private readonly client: ZbxClient,
    readonly contractAddress: string = "0x000000000000000000000000005a42425249444745",
  ) {}

  // ── Views ──────────────────────────────────────────────────────────────────

  /**
   * Get bridging info for a token.
   *
   * @example
   * const info = await zbx.bridge.getTokenInfo("0xUSDC");
   * if (!info.whitelisted) throw new Error("Token not supported");
   * console.log("Max per bridge:", info.maxAmount);
   */
  async getTokenInfo(tokenAddress: string): Promise<BridgeTokenInfo> {
    const addrPad = tokenAddress.slice(2).padStart(64, "0");
    const [rawWl, rawLocked] = await Promise.all([
      this._call(SEL.whitelistedTokens + addrPad),
      this._call(SEL.lockedAmount + addrPad),
    ]);

    const wlBuf      = rawWl.replace(/^0x/, "");
    const whitelisted = h2b(wlBuf.slice(0, 64)) !== 0n;
    const maxWei      = h2b(wlBuf.slice(64, 128));
    const lockedWei   = h2b(rawLocked.replace(/^0x/, "").slice(0, 64));

    return {
      whitelisted,
      maxAmountWei: maxWei,
      maxAmount:    fmtWei(maxWei),
      lockedWei,
      locked:       fmtWei(lockedWei),
    };
  }

  /**
   * Get hourly rate-limit window for a (srcChainId, token) pair.
   *
   * @example
   * const w = await zbx.bridge.getHourlyLimit(1, "0xUSDC");   // from Ethereum
   * console.log("Can bridge:", w.remaining, "USDC this hour");
   */
  async getHourlyLimit(srcChainId: number, tokenAddress: string): Promise<BridgeHourlyWindow> {
    const chainPad = srcChainId.toString(16).padStart(64, "0");
    const addrPad  = tokenAddress.slice(2).padStart(64, "0");

    const [rawLimit, rawVol] = await Promise.all([
      this._call(SEL.bridgeInHourlyLimit + chainPad + addrPad),
      this._call(SEL.bridgeInVolume      + chainPad + addrPad),
    ]);

    const limitWei = h2b(rawLimit.replace(/^0x/, "").slice(0, 64));
    const usedWei  = h2b(rawVol.replace(/^0x/, "").slice(0, 64));
    const remWei   = limitWei > usedWei ? limitWei - usedWei : 0n;

    return {
      limitWei,   limit:     fmtWei(limitWei),
      usedWei,    used:      fmtWei(usedWei),
      remainingWei: remWei,  remaining: fmtWei(remWei),
    };
  }

  /**
   * Get bridge configuration (threshold, relayer count, pause state).
   *
   * @example
   * const cfg = await zbx.bridge.getConfig();
   * console.log(`${cfg.threshold}/${cfg.relayerCount} relayers required`);
   */
  async getConfig(): Promise<{
    threshold: number;
    relayerCount: number;
    paused: boolean;
  }> {
    const [rawT, rawR, rawP] = await Promise.all([
      this._call(SEL.threshold),
      this._call(SEL.relayerCount),
      this._call(SEL.paused),
    ]);
    return {
      threshold:    Number(h2b(rawT.replace(/^0x/, "").slice(0, 64))),
      relayerCount: Number(h2b(rawR.replace(/^0x/, "").slice(0, 64))),
      paused:       h2b(rawP.replace(/^0x/, "").slice(0, 64)) !== 0n,
    };
  }

  /**
   * Check whether a bridge nonce has already been processed (replay guard).
   *
   * @example
   * const used = await zbx.bridge.isNonceProcessed("0xdeadbeef...32bytes");
   * if (used) console.log("Already bridged");
   */
  async isNonceProcessed(nonce: string): Promise<boolean> {
    const raw = await this._call(SEL.processedNonces + nonce.slice(2).padStart(64, "0"));
    return h2b(raw.replace(/^0x/, "").slice(0, 64)) !== 0n;
  }

  // ── Transaction encoders ───────────────────────────────────────────────────

  /**
   * Encode `bridgeOut(address token, uint256 amount, bytes targetAddress)` calldata.
   *
   * `targetAddress` is the recipient address on the destination chain,
   * ABI-encoded as a dynamic bytes field.
   *
   * @example
   * const data = zbx.bridge.encodeBridgeOut(
   *   "0xUSDC",
   *   "100",
   *   "0xYourEthAddress",   // destination address
   * );
   */
  encodeBridgeOut(
    tokenAddress: string,
    amountFormatted: string,
    destinationAddress: string,
  ): string {
    const tokenPad = tokenAddress.slice(2).padStart(64, "0");
    const amountPad = pWei(amountFormatted).toString(16).padStart(64, "0");

    // Dynamic bytes: offset (64 = 0x40), length (20 bytes), padded addr
    const offset    = "0000000000000000000000000000000000000000000000000000000000000060";
    const byteLen   = "0000000000000000000000000000000000000000000000000000000000000014";
    const addrPad   = destinationAddress.slice(2).toLowerCase().padEnd(64, "0");

    return SEL.bridgeOut + tokenPad + amountPad + offset + byteLen + addrPad;
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
