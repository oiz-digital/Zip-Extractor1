/**
 * zebvix.js — Full-stack JavaScript/TypeScript SDK for ZBX chain.
 *
 * Zero dependencies — works in browser, Node.js, Deno, Bun.
 *
 * @example
 * import {
 *   ZbxClient,      // RPC client
 *   ZbxWallet,      // Sign & send txs
 *   ZbxSubscriber,  // WebSocket events
 *   ZbxBatch,       // Batch RPC
 *   ZbxContract,    // ABI contract calls
 *   AmmHelper,      // AMM/LP pool
 *   AaHelper,       // Account Abstraction (EIP-4337)
 *   StakingHelper,  // ZbxStaking
 *   VaultHelper,    // ZusdVault CDP
 *   PerpHelper,     // ZbxPerpetuals
 *   BridgeHelper,   // ZbxBridge cross-chain
 *   MemeHelper,     // ZbxMemeFactory
 *   LendingHelper,  // ZbxLendingPool
 * } from "zebvix.js";
 */

// Core
export { ZbxClient }       from "./client";
export { ZbxWallet }       from "./wallet";

// Pay ID, ZUSD, ZVM helpers (via client.payId / client.zusd / client.zvm)
export { PayIdHelper }     from "./payid";
export { ZusdHelper }      from "./zusd";
export { ZvmHelper }       from "./zvm";
export { ZbxCrypto }       from "./crypto";

// Advanced
export { ZbxSubscriber }   from "./subscribe";
export { ZbxBatch }        from "./batch";
export { ZbxContract, PendingTx }  from "./contract";
export { FeeHelper }       from "./fee";
export { AmmHelper }       from "./amm";
export { AaHelper }        from "./aa";

// v1.2 — New protocol helpers
export { StakingHelper }   from "./staking";
export { VaultHelper }     from "./vault";
export { PerpHelper }      from "./perp";
export { BridgeHelper }    from "./bridge";
export { MemeHelper }      from "./meme";
export { LendingHelper }   from "./lending";

// Middleware
export { logger, cache, retry, rateLimit } from "./middleware";

// Errors
export {
  ZbxError,
  ZbxPayIdNotFoundError,
  ZbxPayIdInvalidError,
  ZbxPayIdTakenError,
  ZbxInsufficientBalanceError,
  ZbxRevertError,
  ZbxRpcError,
  ZbxTimeoutError,
  ZbxSlippageError,
  ZbxContractRevertError,
  ZbxUserOpError,
  ZbxLiquidationError,
  ZbxBridgeError,
  ZbxPerpError,
} from "./errors";

// Types
export type {
  ZbxClientOptions,
  SendResult,
  BlockInfo,
  TxInfo,
  PayIdRecord,
  PoolInfo,
  ChainInfo,
} from "./types";
export type { TxReceipt, TxLog }   from "./receipt";
export type { UserOperation, UserOpReceipt } from "./aa";
export type { FeeEstimate }        from "./fee";
export type { PoolState, SwapQuote, LiquidityQuote } from "./amm";
export type { AbiItem, AbiParam }  from "./contract";
export type { BlockCallback, TxCallback, LogCallback, ZvmLogEntry } from "./subscribe";

// v1.2 — New protocol types
export type { StakeInfo, StakingStats }             from "./staking";
export type { CDP, VaultStats, MintQuote }          from "./vault";
export type {
  MarketInfo,
  Position,
  CrossAccountState,
  OpenPositionParams,
  PnlQuote,
}                                                   from "./perp";
export { PERP_CONSTANTS }                           from "./perp";
export type { BridgeTokenInfo, BridgeHourlyWindow } from "./bridge";
export type { MemeInfo, MemeBuyQuote, MemeSellQuote } from "./meme";
export type { AccountData, MarketData }             from "./lending";

/** SDK version. */
export const VERSION = "1.2.0";

// Re-export the canonical chain ID matrix (single source of truth).
export {
  CHAIN_ID_MAINNET,
  CHAIN_ID_TESTNET,
  BIP44_COIN_TYPE_ZBX,
  DEFAULT_CHAIN_ID,
} from "./constants";
import { CHAIN_ID_MAINNET as _MAINNET } from "./constants";

/** @deprecated Alias kept for one minor cycle; defaults to mainnet. */
export const CHAIN_ID = _MAINNET;

/** ZBX chain constants (mainnet defaults). For testnet/devnet, override `CHAIN_ID`. */
export const ZBX = {
  CHAIN_ID:       _MAINNET,
  CHAIN_NAME:     "Zebvix",
  TOKEN:          "ZBX",
  DECIMALS:       18,
  BLOCK_TIME_MS:  5000,
  ZVM_VERSION:    1,
  MAINNET_RPC:    "https://rpc.zebvix.io",
  MAINNET_WS:     "wss://ws.zebvix.io",
  TESTNET_RPC:    "https://rpc-testnet.zebvix.io",
  TESTNET_WS:     "wss://ws-testnet.zebvix.io",
  EXPLORER:       "https://explorer.zebvix.io",
  WEI_PER_ZBX:    10n ** 18n,

  // Core infra
  REGISTRY:       "0x7e4a7f8bCE8CfD1765CdE34Dbf5e8D7fB1A43e9",
  AMM_POOL:       "0x0000000000000000000000005A42414d4d000000",
  ENTRYPOINT:     "0x5FF137D4b0FDCD49DcA30c7CF57E578a026d2789",

  // v1.2 — Protocol contract addresses (ZEP canonical)
  STAKING:        "0x000000000000000000000000005a425354414b45",
  VAULT:          "0x000000000000000000000000005a425641554c54",
  PERPS:          "0x000000000000000000000000005a425045525053",
  BRIDGE:         "0x000000000000000000000000005a42425249444745",
  MEME_FACTORY:   "0x000000000000000000000000005a424d454d4521",
  LENDING_POOL:   "0x000000000000000000000000005a424c454e4421",
} as const;
