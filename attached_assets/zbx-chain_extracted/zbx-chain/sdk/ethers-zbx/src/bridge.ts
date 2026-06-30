/**
 * Bridge — ZbxBridge cross-chain transfer wrapper for @zebvix/ethers.
 *
 * @example
 * import { Bridge, ZbxProvider, ZbxWallet, ethers } from "@zebvix/ethers";
 *
 * const provider = new ZbxProvider();
 * const wallet   = new ZbxWallet(privateKey, provider);
 * const bridge   = new Bridge(provider);
 *
 * // Check if a token is whitelisted
 * const info = await bridge.getTokenInfo("0xUSDC");
 * if (!info.whitelisted) throw new Error("Token not supported");
 *
 * // Bridge 100 USDC to Ethereum address
 * const tx = await wallet.sendTransaction({
 *   to:   bridge.address,
 *   data: bridge.encodeBridgeOut(
 *     "0xUSDC",
 *     ethers.parseEther("100"),
 *     "0xYourEthAddress",
 *   ),
 * });
 */
import { Contract, Interface, AbiCoder, type Provider, type Signer } from "ethers";

const ABI = [
  // Views
  "function whitelistedTokens(address) view returns (bool whitelisted, uint256 maxAmount)",
  "function lockedAmount(address token) view returns (uint256)",
  "function bridgeInHourlyLimit(uint256 srcChainId, address token) view returns (uint256)",
  "function bridgeInVolume(uint256 srcChainId, address token) view returns (uint256)",
  "function threshold() view returns (uint256)",
  "function relayerCount() view returns (uint256)",
  "function processedNonces(bytes32 nonce) view returns (bool)",
  "function paused() view returns (bool)",
  // Writes
  "function bridgeOut(address token, uint256 amount, bytes targetAddress)",
];

export interface BridgeTokenInfo {
  whitelisted:   boolean;
  maxAmountWei:  bigint;
  lockedWei:     bigint;
}

export interface BridgeWindow {
  limitWei:     bigint;
  usedWei:      bigint;
  remainingWei: bigint;
}

export interface BridgeConfig {
  threshold:    bigint;
  relayerCount: bigint;
  paused:       boolean;
}

export class Bridge {
  readonly address: string;
  private readonly iface: Interface;
  private readonly contract: Contract;
  private readonly coder: AbiCoder;

  constructor(
    providerOrSigner: Provider | Signer,
    address = "0x000000000000000000000000005a42425249444745",
  ) {
    this.address  = address;
    this.iface    = new Interface(ABI);
    this.contract = new Contract(address, ABI, providerOrSigner);
    this.coder    = AbiCoder.defaultAbiCoder();
  }

  /** Get token whitelisting info and current locked amount. */
  async getTokenInfo(token: string): Promise<BridgeTokenInfo> {
    const [info, locked] = await Promise.all([
      this.contract.whitelistedTokens(token),
      this.contract.lockedAmount(token),
    ]);
    return {
      whitelisted:  info.whitelisted as boolean,
      maxAmountWei: info.maxAmount as bigint,
      lockedWei:    locked as bigint,
    };
  }

  /** Get hourly rate-limit window for a (srcChainId, token) pair. */
  async getHourlyWindow(srcChainId: number, token: string): Promise<BridgeWindow> {
    const [limit, used] = await Promise.all([
      this.contract.bridgeInHourlyLimit(srcChainId, token),
      this.contract.bridgeInVolume(srcChainId, token),
    ]);
    const l = limit as bigint;
    const u = used as bigint;
    return {
      limitWei:     l,
      usedWei:      u,
      remainingWei: l > u ? l - u : 0n,
    };
  }

  /** Get bridge configuration. */
  async getConfig(): Promise<BridgeConfig> {
    const [t, r, p] = await Promise.all([
      this.contract.threshold(),
      this.contract.relayerCount(),
      this.contract.paused(),
    ]);
    return {
      threshold:    t as bigint,
      relayerCount: r as bigint,
      paused:       p as boolean,
    };
  }

  /** Check whether a bridge nonce has been processed. */
  async isNonceProcessed(nonce: string): Promise<boolean> {
    return this.contract.processedNonces(nonce) as Promise<boolean>;
  }

  // ── Calldata encoders ────────────────────────────────────────────────────

  /**
   * Encode `bridgeOut(address, uint256, bytes)` calldata.
   * `destinationAddress` is the recipient on the target chain (ABI-encoded as bytes).
   */
  encodeBridgeOut(token: string, amountWei: bigint, destinationAddress: string): string {
    const destBytes = this.coder.encode(["address"], [destinationAddress]);
    return this.iface.encodeFunctionData("bridgeOut", [token, amountWei, destBytes]);
  }
}
