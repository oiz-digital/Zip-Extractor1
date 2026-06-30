/// Zebvix Chain constants.

/// Mainnet chain ID.
const int chainIdMainnet = 8989;

/// Testnet chain ID.
const int chainIdTestnet = 8990;

/// Devnet chain ID.
const int chainIdDevnet = 8991;

/// Decimal places for ZBX (same as ETH = 18).
const int zbxDecimals = 18;

/// Wei per ZBX.
final BigInt weiPerZbx = BigInt.from(10).pow(18);

/// Default gas limit for ZBX transfers.
const int defaultGasTransfer = 21000;

/// Default gas limit for contract interactions.
const int defaultGasContract = 200000;

/// Staking contract address (testnet).
const String stakingContractTestnet = '0x0000000000000000000000000000000000001000';

/// Bridge contract address (testnet).
const String bridgeContractTestnet = '0x0000000000000000000000000000000000001001';
