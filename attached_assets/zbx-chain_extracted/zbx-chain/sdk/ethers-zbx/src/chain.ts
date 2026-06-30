/**
 * ZBX chain definitions for ethers.js v6.
 *
 * Use with `Network.register` or pass to `ZbxProvider`.
 *
 * S13-CHAIN-ID-DRIFT (2026-05-01): chain IDs renumbered to the production
 * matrix — mainnet=8989, testnet=8990 (also used for devnet). Pre-launch
 * IDs (7878/7879) are RETIRED.
 */
import { Network } from "ethers";

/** Zebvix Mainnet chain ID. */
export const CHAIN_ID_MAINNET = 8989 as const;
/** Zebvix Testnet chain ID — also used for devnet. */
export const CHAIN_ID_TESTNET = 8990 as const;

/** @deprecated Use `CHAIN_ID_MAINNET`. Bigint kept for one minor cycle. */
export const ZBX_CHAIN_ID = BigInt(CHAIN_ID_MAINNET);

/** ZBX Mainnet network config */
export const zbxMainnet = {
  name:    "zebvix",
  chainId: CHAIN_ID_MAINNET,
  rpc:     "https://rpc.zebvix.io",
  ws:      "wss://ws.zebvix.io",
  explorer: "https://explorer.zebvix.io",
  nativeCurrency: {
    name:     "Zebvix",
    symbol:   "ZBX",
    decimals: 18,
  },
  vm: "ZVM v1",
  blockTime: 5000, // ms
};

/** ZBX Testnet network config */
export const zbxTestnet = {
  name:     "zebvix-testnet",
  chainId:  CHAIN_ID_TESTNET,
  rpc:      "https://rpc-testnet.zebvix.io",
  ws:       "wss://ws-testnet.zebvix.io",
  explorer: "https://explorer-testnet.zebvix.io",
  nativeCurrency: {
    name:     "Zebvix Testnet",
    symbol:   "tZBX",
    decimals: 18,
  },
  vm: "ZVM v1",
  blockTime: 5000,
};

/** Register ZBX chain with ethers Network */
export function registerZbxNetwork(): void {
  Network.register("zebvix",         () => new Network("zebvix",         CHAIN_ID_MAINNET));
  Network.register("zebvix-testnet", () => new Network("zebvix-testnet", CHAIN_ID_TESTNET));
}