/** ZBX chain block (from zbx_getBlockByNumber) */
export interface ZbxBlock {
  height:       number;
  hash:         string;
  parentHash:   string;
  timestamp:    number;
  transactions: ZbxTransaction[];
  proposer:     string;
  stateRoot:    string;
  receiptRoot:  string;
  gasUsed:      bigint;
  gasLimit:     bigint;
}

/** ZBX transaction */
export interface ZbxTransaction {
  hash:      string;
  from:      string;
  to:        string | null;
  value:     bigint;
  fee:       bigint;
  nonce:     number;
  data:      string;
  kind:      "transfer" | "contract" | "burn" | "payid_register";
  blockHash: string;
  blockHeight: number;
  status:    "success" | "failed" | "pending";
}

/** Pay ID registration info */
export interface ZbxPayIdInfo {
  payId:           string;   // e.g. "ali@zbx"
  address:         string;   // 0x...
  registeredBlock: number;
  expiryBlock:     number | null;
}

/** AMM pool state */
export interface ZbxPoolState {
  initialized:       boolean;
  poolAddress:       string;
  zbxReserveWei:     string;
  zusdReserve:       string;
  lpSupply:          string;
  spotPriceUsdPerZbx: string;
  feeAccZbx:         string;
  feeAccZusd:        string;
}

/** ZBX chain info */
export interface ZbxChainInfo {
  chainId:       number;
  chainName:     string;
  token:         string;
  vm:            string;
  blockTimeSecs: number;
  tipHeight:     number;
}

/** ZVM execution result */
export interface ZvmResult {
  status:     "success" | "revert" | "oog" | "invalid";
  returnData: string;
  gasUsed:    bigint;
}

// ── v1.2 Protocol types ───────────────────────────────────────────────────────

/** ZbxStaking user info */
export interface ZbxStakeInfo {
  /** Currently staked (wei). */
  stakedWei:  bigint;
  /** Pending unclaimed reward (wei). */
  pendingWei: bigint;
  /** UNIX timestamp of last stake action. */
  lastUpdate: number;
  /** Whether the MIN_STAKE_AGE (1h) has elapsed since last stake. */
  claimable:  boolean;
}

/** ZusdVault CDP state */
export interface ZbxCDPState {
  collateral:   bigint;
  debt:         bigint;
  currentDebt:  bigint;
  crBps:        bigint;
  exists:       boolean;
  liquidatable: boolean;
}

/** ZbxPerpetuals market summary */
export interface ZbxPerpMarket {
  symbol:        string;
  oracle:        string;
  active:        boolean;
  maxLeverage:   bigint;
  totalLongOI:   bigint;
  totalShortOI:  bigint;
  fundingRate:   bigint;
  nextFundingIn: bigint;
  markPrice:     bigint;
}

/** ZbxBridge token config */
export interface ZbxBridgeToken {
  whitelisted:  boolean;
  maxAmountWei: bigint;
  lockedWei:    bigint;
}