/**
 * FeeHelper — gas/fee estimation for ZBX chain.
 * Accessible via `client.fee.*`
 *
 * @example
 * // Estimate fee for a simple ZBX transfer
 * const fee = await zbx.fee.estimateTransfer("0xFrom...", "0xTo...", "100");
 * console.log("Estimated fee:", fee.feeZbx, "ZBX");
 * console.log("Gas limit:", fee.gasLimit);
 *
 * // Estimate fee for a contract call
 * const fee = await zbx.fee.estimateCall("0xContract...", calldata, "0xFrom...");
 *
 * // Get current base fee
 * const baseFee = await zbx.fee.baseFee();
 */
import type { ZbxClient } from "./client";

export interface FeeEstimate {
  gasLimit:   bigint;
  gasPrice:   bigint;
  feeWei:     bigint;
  feeZbx:     string;
  maxFeeWei:  bigint;  // 20% buffer
}

export class FeeHelper {
  constructor(private readonly client: ZbxClient) {}

  /**
   * Get current base fee per gas (ZBX).
   *
   * @example
   * const base = await zbx.fee.baseFee();
   * console.log("Base fee:", base.toString(), "wei/gas");
   */
  async baseFee(): Promise<bigint> {
    const block = await this.client.rpc<{ baseFeePerGas: string }>("eth_getBlockByNumber", ["latest", false]);
    return BigInt(block.baseFeePerGas || "0");
  }

  /**
   * Get recommended gas price (base fee + priority tip).
   *
   * @example
   * const price = await zbx.fee.gasPrice();
   */
  async gasPrice(): Promise<bigint> {
    const hex = await this.client.rpc<string>("eth_gasPrice");
    return BigInt(hex);
  }

  /**
   * Estimate fee for a ZBX transfer.
   *
   * @example
   * const est = await zbx.fee.estimateTransfer("0xFrom...", "0xTo...", "100");
   * console.log("Fee:", est.feeZbx, "ZBX");
   * console.log("Gas:", est.gasLimit.toString());
   */
  async estimateTransfer(from: string, to: string, amountZbx: string): Promise<FeeEstimate> {
    // SEC-2026-05-09 (S4): use exact decimal-string parsing for value.
    // The previous `parseFloat(amountZbx) * 1e18` lost precision for any
    // amount with more than ~15 significant digits — e.g. parsing
    // "1.234567890123456789" rounded down to ~1.234567890123456 ZBX, so the
    // returned fee estimate was for a smaller transfer than the user
    // intended. With token amounts denominated in 18-decimal wei, that
    // silent rounding could mask multi-cent differences on large transfers.
    return this.estimateCall(to, "0x", from, parseWei(amountZbx));
  }

  /**
   * Estimate fee for a contract call.
   *
   * @example
   * const est = await zbx.fee.estimateCall("0xContract...", "0xabcd...", "0xFrom...");
   */
  async estimateCall(
    to:    string,
    data:  string,
    from?: string,
    value?: bigint,
  ): Promise<FeeEstimate> {
    const [gasLimitHex, gasPrice] = await Promise.all([
      this.client.rpc<string>("eth_estimateGas", [{
        from: from ?? "0x0000000000000000000000000000000000000000",
        to,
        data,
        value: value ? "0x" + value.toString(16) : "0x0",
      }]),
      this.gasPrice(),
    ]);

    const gasLimit = BigInt(gasLimitHex) * 12n / 10n; // +20% buffer
    const feeWei   = gasLimit * gasPrice;
    const maxFeeWei = feeWei * 12n / 10n;             // +20% max fee
    const feeZbx   = formatWei(feeWei);

    return { gasLimit, gasPrice, feeWei, feeZbx, maxFeeWei };
  }

  /**
   * Estimate fee for Pay ID registration (1 ZBX + gas).
   *
   * @example
   * const est = await zbx.fee.estimatePayIdRegister("0xFrom...");
   * console.log("Total cost:", est.totalZbx, "ZBX");
   */
  async estimatePayIdRegister(from: string): Promise<FeeEstimate & { totalZbx: string }> {
    const registryAddress = "0x7e4a7f8bCE8CfD1765CdE34Dbf5e8D7fB1A43e9"; // ZEP-001 canonical
    const est = await this.estimateCall(
      registryAddress,
      "0x5a425041" + "0".repeat(64),
      from,
      10n ** 16n, // 0.01 ZBX (ZEP-001 canonical fee)
    );
    const totalWei = est.feeWei + 10n ** 16n; // gas fee + 0.01 ZBX registration
    return { ...est, totalZbx: formatWei(totalWei) };
  }
}

/**
 * SEC-2026-05-09 (S4): exact decimal-string → wei parser.
 * Handles up to 18 fractional digits without ever going through `Number`.
 */
function parseWei(amount: string): bigint {
  const trimmed = amount.trim();
  if (!/^[0-9]+(\.[0-9]+)?$/.test(trimmed)) {
    throw new Error(`S4: invalid decimal amount: "${amount}"`);
  }
  const [whole, frac = ""] = trimmed.split(".");
  const fracPadded = frac.padEnd(18, "0").slice(0, 18);
  return BigInt(whole) * 10n ** 18n + (fracPadded ? BigInt(fracPadded) : 0n);
}

function formatWei(wei: bigint): string {
  const whole = wei / 10n ** 18n;
  const frac  = wei % 10n ** 18n;
  const fracStr = frac.toString().padStart(18, "0").slice(0, 6).replace(/0+$/, "");
  return fracStr ? `\${whole}.\${fracStr}` : whole.toString();
}