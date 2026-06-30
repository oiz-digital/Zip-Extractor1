/**
 * Locked chain ID matrix — single source of truth for the entire SDK.
 *
 * S13-CHAIN-ID-DRIFT (2026-05-01): IDs were renumbered from the pre-launch
 * matrix (7878/7879) to the production matrix (8989 mainnet / 8990 shared
 * testnet+devnet) to align RPC, wallet, faucet, explorer, and bridge.
 *
 * Importers MUST use these constants. Inline literals will be rejected by CI
 * (`scripts/check-chain-id.sh`).
 */

/** Zebvix Mainnet chain ID. */
export const CHAIN_ID_MAINNET = 8989 as const;

/** Zebvix Testnet chain ID — also used for devnet (private testnet). */
export const CHAIN_ID_TESTNET = 8990 as const;

/** SLIP-44 registered coin type for Zebvix. INDEPENDENT of chain ID. */
export const BIP44_COIN_TYPE_ZBX = 7878 as const;

/** Default chain ID for new wallets/transactions when none is specified. */
export const DEFAULT_CHAIN_ID = CHAIN_ID_MAINNET;
