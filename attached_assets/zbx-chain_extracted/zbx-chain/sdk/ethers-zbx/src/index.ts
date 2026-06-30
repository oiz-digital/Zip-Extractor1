/**
 * @zebvix/ethers — Ethers.js v6 + ZBX chain extensions.
 *
 * Drop-in replacement for ethers when working with ZBX chain.
 * All standard ethers exports work unchanged, plus ZBX-specific additions.
 *
 * @example
 * import {
 *   ZbxProvider, ZbxWallet,
 *   PayId, ZUSD, ZvmClient,
 *   Staking, Perps, Vault, Bridge,
 *   zbxMainnet,
 * } from "@zebvix/ethers";
 * import { ethers } from "@zebvix/ethers"; // re-export of ethers
 *
 * const provider = new ZbxProvider("https://rpc.zebvix.io");
 * const wallet   = new ZbxWallet(privateKey, provider);
 *
 * // Standard ethers — works unchanged
 * const balance = await wallet.getBalance();
 *
 * // v1.0 ZBX extensions
 * const address = await PayId.resolve("ali@zbx", provider);
 * const zusdBal = await ZUSD.balanceOf(addr, provider);
 * const price   = await provider.zbx.price();
 *
 * // v1.2 Protocol helpers
 * const staking = new Staking(wallet);
 * const info    = await staking.getStakeInfo("0xYourAddress");
 *
 * const perps   = new Perps(wallet);
 * const market  = await perps.getMarket(0); // BTC-USD
 *
 * const vault   = new Vault(wallet);
 * const cdp     = await vault.getCDP("0xYourAddress");
 *
 * const bridge  = new Bridge(provider);
 * const tkInfo  = await bridge.getTokenInfo("0xUSDC");
 */

// Re-export all of ethers unchanged
export * from "ethers";

// ZBX chain config
export { zbxMainnet, zbxTestnet, ZBX_CHAIN_ID } from "./chain";

// ZBX-extended provider
export { ZbxProvider } from "./provider";

// ZBX-extended wallet
export { ZbxWallet } from "./wallet";

// Pay ID utilities
export { PayId } from "./payid";

// ZUSD token helper
export { ZUSD } from "./zusd";

// ZVM utilities
export { ZvmClient } from "./zvm";

// v1.2 — Protocol contract wrappers
export { Staking } from "./staking";
export { Perps }   from "./perps";
export { Vault }   from "./vault";
export { Bridge }  from "./bridge";

// Types
export type {
  ZbxBlock,
  ZbxTransaction,
  ZbxPayIdInfo,
  ZbxPoolState,
  ZbxChainInfo,
  ZvmResult,
} from "./types";

// v1.2 — Protocol types
export type { StakeInfo, StakingStats }                          from "./staking";
export type {
  PerpMarket,
  PerpPosition,
  CrossAccountState,
  OpenPositionParams,
  PnlQuote,
}                                                                from "./perps";
export { PERP_CONSTANTS }                                        from "./perps";
export type { CDPState, VaultStats }                             from "./vault";
export type { BridgeTokenInfo, BridgeWindow, BridgeConfig }      from "./bridge";

/** SDK version. */
export const ETHERS_ZBX_VERSION = "1.2.0";
