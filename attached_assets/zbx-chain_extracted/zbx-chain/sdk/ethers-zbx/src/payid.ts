/**
 * Pay ID utilities for ZBX chain.
 *
 * @example
 * import { PayId } from "@zebvix/ethers";
 *
 * // Resolve
 * const addr = await PayId.resolve("ali@zbx", provider);
 *
 * // Validate format
 * PayId.validate("alice@zbx"); // throws if invalid
 *
 * // Parse
 * const { name, handle } = PayId.parse("ali@zbx");
 * // → { name: "ali", handle: "zbx" }
 */
import { ZbxProvider } from "./provider";
import type { ZbxPayIdInfo } from "./types";

export const PayId = {

  /**
   * Resolve a Pay ID to a wallet address.
   * Returns null if not registered.
   *
   * @example
   * const address = await PayId.resolve("ali@zbx", provider);
   * if (!address) throw new Error("Pay ID not found");
   */
  async resolve(payId: string, provider: ZbxProvider): Promise<string | null> {
    PayId.validate(payId);
    return provider.resolvePayId(payId);
  },

  /**
   * Get full Pay ID info for an address (reverse lookup).
   * Returns null if address has no Pay ID.
   *
   * @example
   * const info = await PayId.infoOf("0x742d35...", provider);
   * console.log(info?.payId); // "ali@zbx"
   */
  async infoOf(address: string, provider: ZbxProvider): Promise<ZbxPayIdInfo | null> {
    return provider.zbx.payIdOf(address);
  },

  /**
   * Check if a Pay ID is available (not registered).
   *
   * @example
   * if (await PayId.isAvailable("alice@zbx", provider)) {
   *   await wallet.registerPayId("alice@zbx");
   * }
   */
  async isAvailable(payId: string, provider: ZbxProvider): Promise<boolean> {
    const addr = await PayId.resolve(payId, provider);
    return addr === null;
  },

  /**
   * Validate Pay ID format. Throws if invalid.
   *
   * Rules:
   * - Must end with @zbx
   * - Name part: 2–32 chars
   * - Allowed: a-z, 0-9, _, -
   *
   * @example
   * PayId.validate("ali@zbx");    // OK
   * PayId.validate("ali@eth");    // throws: must end with @zbx
   * PayId.validate("a@zbx");      // throws: too short
   * PayId.validate("ALI@zbx");    // throws: uppercase not allowed
   */
  validate(payId: string): void {
    if (!payId.endsWith("@zbx")) {
      throw new Error(`Invalid Pay ID: '\${payId}' — must be <name>@zbx (e.g. ali@zbx)`);
    }
    const name = payId.slice(0, -4);
    if (name.length < 2 || name.length > 32) {
      throw new Error(`Pay ID name must be 2–32 characters (got \${name.length})`);
    }
    if (!/^[a-z0-9_-]+$/.test(name)) {
      throw new Error(`Pay ID name can only contain a-z, 0-9, _, - (got: '\${name}')`);
    }
  },

  /**
   * Parse a Pay ID into name and handle.
   *
   * @example
   * const { name, handle } = PayId.parse("ali@zbx");
   * // { name: "ali", handle: "zbx" }
   */
  parse(payId: string): { name: string; handle: string } {
    const at = payId.lastIndexOf("@");
    if (at === -1) throw new Error(`Invalid Pay ID: '\${payId}'`);
    return {
      name:   payId.slice(0, at),
      handle: payId.slice(at + 1),
    };
  },

  /**
   * Format an address to a Pay ID display string.
   * Returns short address if no Pay ID registered.
   *
   * @example
   * await PayId.display("0x742d35Cc...", provider);
   * // "ali@zbx" if registered, else "0x742d...Cc"
   */
  async display(address: string, provider: ZbxProvider): Promise<string> {
    const info = await PayId.infoOf(address, provider);
    if (info?.payId) return info.payId;
    return address.slice(0, 6) + "..." + address.slice(-4);
  },

  /** Returns true if string looks like a Pay ID. */
  isPayId(value: string): boolean {
    return value.endsWith("@zbx") && value.length > 4;
  },
};