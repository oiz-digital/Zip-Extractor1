"""Zebvix Chain constants."""

#: Mainnet chain ID
CHAIN_ID_MAINNET: int = 8989
#: Testnet chain ID
CHAIN_ID_TESTNET: int = 8990
#: Devnet chain ID
CHAIN_ID_DEVNET: int  = 8991

#: Decimal places for ZBX (same as ETH = 18)
ZBX_DECIMALS: int = 18
#: Wei per ZBX
WEI_PER_ZBX: int = 10 ** ZBX_DECIMALS

#: Default gas limit for simple ZBX transfers
DEFAULT_GAS_TRANSFER: int = 21_000
#: Default gas limit for contract interactions
DEFAULT_GAS_CONTRACT: int = 200_000

#: Staking contract address (Zebvix testnet)
STAKING_CONTRACT_TESTNET: str = "0x0000000000000000000000000000000000001000"
#: Bridge contract address (Zebvix testnet)
BRIDGE_CONTRACT_TESTNET: str  = "0x0000000000000000000000000000000000001001"
#: PayID contract address (Zebvix testnet)
PAYID_CONTRACT_TESTNET: str   = "0x0000000000000000000000000000000000001002"
