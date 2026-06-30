/**
 * PayIdHelper — Pay ID resolution and management.
 * Accessible via `client.payId.*`
 */
import type { ZbxClient } from "./client";
import type { PayIdRecord } from "./types";

export class PayIdHelper {
  constructor(private client: ZbxClient) {}

  /**
   * Resolve a Pay ID to a wallet address.
   * Returns null if not registered.
   *
   * @example
   * const address = await zbx.payId.resolve("ali@zbx");
   * // "0x742d35Cc6634C0532925a3b844Bc454e4438f44e" or null
   */
  async resolve(payId: string): Promise<string | null> {
    this.validate(payId);
    const addr = await this.client.rpc<string>("zbx_resolvePayId", [payId]);
    if (!addr || addr === "0x0000000000000000000000000000000000000000") return null;
    return addr;
  }

  /**
   * Get Pay ID registered to an address (reverse lookup).
   * Returns null if address has no Pay ID.
   *
   * @example
   * const payId = await zbx.payId.of("0x742d35...");
   * // "ali@zbx" or null
   */
  async of(address: string): Promise<string | null> {
    const info = await this.client.rpc<PayIdRecord | null>("zbx_getPayIdForAddress", [address]);
    return info?.payId ?? null;
  }

  /**
   * Check if a Pay ID is available (not yet registered).
   *
   * @example
   * const available = await zbx.payId.isAvailable("newname@zbx");
   * if (available) await wallet.registerPayId("newname@zbx");
   */
  async isAvailable(payId: string): Promise<boolean> {
    return (await this.resolve(payId)) === null;
  }

  /**
   * Get full Pay ID record with registration details.
   *
   * @example
   * const record = await zbx.payId.record("ali@zbx");
   * console.log(record?.registeredBlock); // 50123
   */
  async record(payId: string): Promise<PayIdRecord | null> {
    const addr = await this.resolve(payId);
    if (!addr) return null;
    return this.client.rpc("zbx_getPayIdRecord", [payId]);
  }

  /** Validate Pay ID format. Throws if invalid. */
  validate(payId: string): void {
    if (!payId.endsWith("@zbx")) {
      throw new Error(`Invalid Pay ID: must be <name>@zbx (got '\${payId}')`);
    }
    const name = payId.slice(0, -4);
    if (name.length < 2 || name.length > 32) {
      throw new Error(`Pay ID name must be 2–32 chars (got \${name.length})`);
    }
    if (!/^[a-z0-9_-]+$/.test(name)) {
      throw new Error(`Pay ID: only lowercase a-z, 0-9, _, - allowed (got '\${name}')`);
    }
  }

  /** Returns true if string is a valid Pay ID format. */
  isPayId(value: string): boolean {
    try { this.validate(value); return true; } catch { return false; }
  }

  /** Parse Pay ID into name + handle parts. */
  parse(payId: string): { name: string; handle: string } {
    const at = payId.lastIndexOf("@");
    return { name: payId.slice(0, at), handle: payId.slice(at + 1) };
  }
}