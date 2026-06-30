export interface ZbxClientOptions {
  timeout?: number;
  headers?: Record<string, string>;
}

export interface SendResult {
  hash:      string;
  from:      string;
  to:        string;
  amountWei: bigint;
  amountZbx: string;
}

export interface BlockInfo {
  height:       number;
  hash:         string;
  parentHash:   string;
  timestamp:    number;
  txCount:      number;
  proposer:     string;
  stateRoot:    string;
  gasUsed:      string;
}

export interface TxInfo {
  hash:        string;
  from:        string;
  to:          string;
  value:       string;
  fee:         string;
  nonce:       number;
  status:      "success" | "failed" | "pending";
  blockHeight: number;
  kind:        string;
}

export interface PayIdRecord {
  payId:           string;
  address:         string;
  registeredBlock: number;
  expiryBlock:     number | null;
}

export interface PoolInfo {
  initialized:        boolean;
  poolAddress:        string;
  zbxReserveWei:      string;
  zusdReserve:        string;
  lpSupply:           string;
  spotPriceUsdPerZbx: string;
}

export interface ChainInfo {
  chainId:       number;
  chainName:     string;
  token:         string;
  vm:            string;
  blockTimeSecs: number;
  tipHeight:     number;
}