/**
 * ZbxProvider — extends ethers.JsonRpcProvider with ZBX-specific methods.
 *
 * Standard ethers methods all work unchanged.
 * ZBX-specific methods live under `provider.zbx.*`.
 */
import { JsonRpcProvider, Network } from "ethers";
import { zbxMainnet, CHAIN_ID_MAINNET } from "./chain";
import type { ZbxBlock, ZbxPoolState, ZbxChainInfo, ZbxPayIdInfo } from "./types";

export class ZbxProvider extends JsonRpcProvider {
  /** ZBX-specific RPC methods. */
  readonly zbx: ZbxRpcMethods;

  constructor(url?: string, network?: Network | string | number) {
    const rpcUrl = url ?? zbxMainnet.rpc;
    const net = network ?? new Network("zebvix", CHAIN_ID_MAINNET);
    super(rpcUrl, net, { staticNetwork: true });
    this.zbx = new ZbxRpcMethods(this);
  }

  /**
   * Get a ZBX block by height (uses zbx_getBlockByNumber).
   */
  async getZbxBlock(height: number): Promise<ZbxBlock> {
    return this.send("zbx_getBlockByNumber", [height]);
  }

  /**
   * Resolve a Pay ID to an address (uses zbx_resolvePayId).
   * Returns null if not found.
   *
   * @example
   * const addr = await provider.resolvePayId("ali@zbx");
   */
  async resolvePayId(payId: string): Promise<string | null> {
    const addr: string = await this.send("zbx_resolvePayId", [payId]);
    if (!addr || addr === "0x0000000000000000000000000000000000000000") {
      return null;
    }
    return addr;
  }

  /**
   * Override resolveName to support Pay ID format (name@zbx).
   * This means ethers ENS resolution will also work for ZBX Pay IDs.
   *
   * @example
   * // Works automatically when you pass a Pay ID as address:
   * const wallet = new ZbxWallet(key, provider);
   * await wallet.sendTransaction({ to: "ali@zbx", value: 100n }); // auto-resolves!
   */
  override async resolveName(name: string): Promise<string | null> {
    if (name.endsWith("@zbx")) {
      return this.resolvePayId(name);
    }
    return super.resolveName(name);
  }
}

/**
 * ZBX-specific RPC methods, namespaced under `provider.zbx.*`.
 */
class ZbxRpcMethods {
  constructor(private provider: ZbxProvider) {}

  /** Get chain info (VM version, block time, etc.) */
  async info(): Promise<ZbxChainInfo> {
    const [chainId, blockNumber] = await Promise.all([
      this.provider.send("eth_chainId", []),
      this.provider.send("eth_blockNumber", []),
    ]);
    return {
      chainId: parseInt(chainId, 16),
      chainName: "Zebvix",
      token: "ZBX",
      vm: "ZVM v1",
      blockTimeSecs: 5,
      tipHeight: parseInt(blockNumber, 16),
    };
  }

  /** Get current ZBX/USD price from on-chain oracle. */
  async price(): Promise<{ zbxUsd: string; source: string }> {
    return this.provider.send("zbx_getPriceUSD", []);
  }

  /** Get ZUSD balance of an address. */
  async zusdBalance(address: string): Promise<bigint> {
    const raw: string = await this.provider.send("zbx_getZusdBalance", [address]);
    return BigInt(raw || "0");
  }

  /** Get AMM pool state (ZBX/ZUSD reserves, price, etc.) */
  async pool(): Promise<ZbxPoolState> {
    return this.provider.send("zbx_getPool", []);
  }

  /** Resolve a Pay ID → address. Returns null if not found. */
  async resolvePayId(payId: string): Promise<string | null> {
    return this.provider.resolvePayId(payId);
  }

  /** Get Pay ID info for an address (reverse lookup). */
  async payIdOf(address: string): Promise<ZbxPayIdInfo | null> {
    return this.provider.send("zbx_getPayIdForAddress", [address]);
  }

  /** Check if a Pay ID is registered. */
  async isPayIdRegistered(payId: string): Promise<boolean> {
    const addr = await this.resolvePayId(payId);
    return addr !== null;
  }

  /** Get pending nonce for an address. */
  async nonce(address: string): Promise<number> {
    const n = await this.provider.send("zbx_getNonce", [address]);
    return Number(n);
  }

  /** Get LP token balance of an address. */
  async lpBalance(address: string): Promise<bigint> {
    const raw: string = await this.provider.send("zbx_getLpBalance", [address]);
    return BigInt(raw || "0");
  }
}