/**
 * ZbxClient — main entry point for zebvix.js.
 * Wraps all ZBX RPC calls with a clean TypeScript API.
 */
import type { ZbxClientOptions, BlockInfo, TxInfo, ChainInfo, PoolInfo } from "./types";
import { ZbxWallet } from "./wallet";
import { PayIdHelper } from "./payid";
import { ZusdHelper } from "./zusd";
import { ZvmHelper } from "./zvm";
import { StakingHelper } from "./staking";
import { VaultHelper } from "./vault";
import { PerpHelper } from "./perp";
import { BridgeHelper } from "./bridge";
import { MemeHelper } from "./meme";
import { LendingHelper } from "./lending";

export class ZbxClient {
  readonly rpcUrl: string;

  /** Pay ID utilities */
  readonly payId: PayIdHelper;
  /** ZUSD stablecoin utilities */
  readonly zusd: ZusdHelper;
  /** ZVM (Zebvix VM) utilities */
  readonly zvm: ZvmHelper;
  /** ZbxStaking — stake, unstake, claim rewards */
  readonly staking: StakingHelper;
  /** ZusdVault — CDP collateral, mint/repay ZUSD */
  readonly vault: VaultHelper;
  /** ZbxPerpetuals — open/close leveraged positions */
  readonly perp: PerpHelper;
  /** ZbxBridge — cross-chain token transfers */
  readonly bridge: BridgeHelper;
  /** ZbxMemeFactory — launch and trade meme coins */
  readonly meme: MemeHelper;
  /** ZbxLendingPool — supply, borrow, repay, liquidate */
  readonly lending: LendingHelper;

  constructor(rpcUrl?: string, options?: ZbxClientOptions) {
    this.rpcUrl  = rpcUrl ?? "https://rpc.zebvix.io";
    this.payId   = new PayIdHelper(this);
    this.zusd    = new ZusdHelper(this);
    this.zvm     = new ZvmHelper(this);
    this.staking = new StakingHelper(this);
    this.vault   = new VaultHelper(this);
    this.perp    = new PerpHelper(this);
    this.bridge  = new BridgeHelper(this);
    this.meme    = new MemeHelper(this);
    this.lending = new LendingHelper(this);
  }

  // ── Core RPC ────────────────────────────────────────────────────────────────

  /** Low-level JSON-RPC call. */
  async rpc<T = unknown>(method: string, params: unknown[] = []): Promise<T> {
    const body = JSON.stringify({ jsonrpc: "2.0", id: 1, method, params });
    const res  = await fetch(this.rpcUrl, {
      method:  "POST",
      headers: { "Content-Type": "application/json" },
      body,
    });
    if (!res.ok) throw new Error(`HTTP \${res.status}: \${res.statusText}`);
    const json = await res.json() as { result?: T; error?: { message: string } };
    if (json.error) throw new Error(`RPC error: \${json.error.message}`);
    return json.result as T;
  }

  // ── Chain info ──────────────────────────────────────────────────────────────

  /** Get basic chain information. */
  async getChainInfo(): Promise<ChainInfo> {
    const [chainIdHex, blockHex] = await Promise.all([
      this.rpc<string>("eth_chainId"),
      this.rpc<string>("eth_blockNumber"),
    ]);
    return {
      chainId:       parseInt(chainIdHex, 16),
      chainName:     "Zebvix",
      token:         "ZBX",
      vm:            "ZVM v1",
      blockTimeSecs: 5,
      tipHeight:     parseInt(blockHex, 16),
    };
  }

  /** Get latest block height. */
  async getBlockNumber(): Promise<number> {
    const hex = await this.rpc<string>("eth_blockNumber");
    return parseInt(hex, 16);
  }

  // ── Blocks ──────────────────────────────────────────────────────────────────

  /** Get a ZBX block by height. */
  async getBlock(height: number): Promise<BlockInfo> {
    return this.rpc("zbx_getBlockByNumber", [height]);
  }

  /** Get latest block. */
  async getLatestBlock(): Promise<BlockInfo> {
    const height = await this.getBlockNumber();
    return this.getBlock(height);
  }

  // ── Balances ────────────────────────────────────────────────────────────────

  /**
   * Get ZBX balance of an address in wei.
   *
   * @example
   * const bal = await zbx.getBalance("0x742d35...");
   * console.log(zbx.formatZbx(bal)); // "1250.5"
   */
  async getBalance(address: string): Promise<bigint> {
    const hex = await this.rpc<string>("eth_getBalance", [address, "latest"]);
    return BigInt(hex);
  }

  /** Get ZBX balance formatted as a decimal string (e.g. "1250.5"). */
  async getBalanceZbx(address: string): Promise<string> {
    const wei = await this.getBalance(address);
    return this.formatZbx(wei);
  }

  // ── Transactions ────────────────────────────────────────────────────────────

  /** Get transaction by hash. */
  async getTransaction(hash: string): Promise<TxInfo> {
    return this.rpc("zbx_getTransaction", [hash]);
  }

  /** Get pending nonce for an address. */
  async getNonce(address: string): Promise<number> {
    const n = await this.rpc<number>("zbx_getNonce", [address]);
    return Number(n);
  }

  // ── Pool ────────────────────────────────────────────────────────────────────

  /** Get AMM pool state (ZBX/ZUSD). */
  async getPool(): Promise<PoolInfo> {
    return this.rpc("zbx_getPool");
  }

  // ── Wallet factory ──────────────────────────────────────────────────────────

  /**
   * Create a wallet connected to this client.
   *
   * @example
   * const wallet = zbx.wallet(process.env.PRIVATE_KEY);
   * const tx = await wallet.send("ali@zbx", "100");
   */
  wallet(privateKeyHex: string): ZbxWallet {
    return new ZbxWallet(privateKeyHex, this);
  }

  // ── Utilities ────────────────────────────────────────────────────────────────

  /** Format ZBX wei to decimal string. */
  formatZbx(wei: bigint, decimals = 4): string {
    const whole = wei / 10n ** 18n;
    const frac  = wei % 10n ** 18n;
    if (frac === 0n) return whole.toString();
    const fracStr = frac.toString().padStart(18, "0").slice(0, decimals);
    return `\${whole}.\${fracStr.replace(/0+$/, "")}`;
  }

  /** Parse ZBX decimal string to wei. */
  parseZbx(amount: string): bigint {
    const [whole, frac = ""] = amount.split(".");
    const fracPadded = frac.padEnd(18, "0").slice(0, 18);
    return BigInt(whole) * 10n ** 18n + BigInt(fracPadded || "0");
  }
}