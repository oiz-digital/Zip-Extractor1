/**
 * ZbxWallet — extends ethers.Wallet with ZBX-specific features.
 *
 * Standard ethers.Wallet methods all work unchanged.
 * ZBX-specific methods are added directly on the wallet.
 */
import { Wallet, TransactionRequest } from "ethers";
import { ZbxProvider } from "./provider";

export class ZbxWallet extends Wallet {
  declare provider: ZbxProvider;

  constructor(
    privateKey: string | Uint8Array,
    provider?: ZbxProvider,
  ) {
    super(privateKey, provider);
  }

  /**
   * Get ZBX balance of this wallet (alias for getBalance).
   */
  async zbxBalance(): Promise<bigint> {
    return this.provider.getBalance(this.address);
  }

  /**
   * Get ZUSD balance of this wallet.
   */
  async zusdBalance(): Promise<bigint> {
    return this.provider.zbx.zusdBalance(this.address);
  }

  /**
   * Get this wallet's Pay ID (if registered).
   */
  async payId(): Promise<string | null> {
    const info = await this.provider.zbx.payIdOf(this.address);
    return info?.payId ?? null;
  }

  /**
   * Send ZBX to a Pay ID or address.
   *
   * If `to` is a Pay ID (e.g. "ali@zbx"), it is automatically resolved.
   * Uses zbx_sendTransaction for ZBX-specific transaction signing.
   *
   * @example
   * // Send to a Pay ID (auto-resolved)
   * const tx = await wallet.sendZbx("ali@zbx", "100");
   *
   * // Send to a raw address
   * const tx = await wallet.sendZbx("0x742d35Cc...", "50.5");
   */
  async sendZbx(
    to: string,
    amountZbx: string,
    options?: { fee?: bigint },
  ): Promise<{ txHash: string; from: string; to: string; amountWei: bigint }> {
    // Resolve Pay ID if needed
    let toAddress = to;
    if (to.endsWith("@zbx")) {
      const resolved = await this.provider.resolvePayId(to);
      if (!resolved) {
        throw new Error(`Pay ID not found: \${to}`);
      }
      toAddress = resolved;
    }

    // Parse amount
    const amountWei = parseZbx(amountZbx);

    // Get nonce
    const nonce = await this.provider.zbx.nonce(this.address);

    // Build and sign transaction.
    //
    // chainId is taken from the connected provider's network. Hardcoding
    // here would silently break on testnet/devnet (EIP-155 mismatch →
    // every signed tx rejected). See AUDIT §S13.2-B / S13-CHAIN-ID-DRIFT.
    const network = await this.provider.getNetwork();
    const tx: TransactionRequest = {
      to: toAddress,
      value: amountWei,
      nonce,
      chainId: network.chainId,
    };

    const signed = await this.signTransaction(tx);
    const txHash: string = await this.provider.send("zbx_sendTransaction", [signed]);

    return { txHash, from: this.address, to: toAddress, amountWei };
  }

  /**
   * Register a Pay ID for this wallet (costs 1 ZBX).
   *
   * @example
   * const receipt = await wallet.registerPayId("alice@zbx");
   */
  async registerPayId(payId: string): Promise<{ txHash: string; payId: string }> {
    // Validate format
    if (!payId.endsWith("@zbx")) {
      throw new Error(`Invalid Pay ID format: '\${payId}' — must end with @zbx`);
    }
    const name = payId.slice(0, -4);
    if (name.length < 2 || name.length > 32) {
      throw new Error("Pay ID name must be 2–32 characters");
    }
    if (!/^[a-z0-9_-]+$/.test(name)) {
      throw new Error("Pay ID name can only contain a-z, 0-9, _, -");
    }

    const nonce = await this.provider.zbx.nonce(this.address);
    const registryAddress = "0x7e4a7f8bCE8CfD1765CdE34Dbf5e8D7fB1A43e9";

    // chainId from provider's network (anti-S13-CHAIN-ID-DRIFT).
    const network = await this.provider.getNetwork();
    const tx: TransactionRequest = {
      to: registryAddress,
      value: parseZbx("0.01"), // 0.01 ZBX registration fee (ZEP-001)
      nonce,
      chainId: network.chainId,
      data: encodePayIdRegistration(payId),
    };

    const signed = await this.signTransaction(tx);
    const txHash: string = await this.provider.send("zbx_registerPayId", [payId, signed]);

    return { txHash, payId };
  }
}

/** Parse a ZBX amount string to wei (bigint). */
function parseZbx(amount: string): bigint {
  const [whole, frac = ""] = amount.split(".");
  const fracPadded = frac.padEnd(18, "0").slice(0, 18);
  return BigInt(whole) * 10n ** 18n + BigInt(fracPadded);
}

/** Encode a Pay ID registration call. */
function encodePayIdRegistration(payId: string): string {
  const encoded = Buffer.from(payId, "utf8").toString("hex");
  return "0x" + "5a425041" + encoded; // ZbPa selector + payId bytes
}