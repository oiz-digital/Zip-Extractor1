# ZUSD Genesis Additions

The following is added to testnet-genesis.json for ZUSD launch:

```json
"zusd_config": {
  "min_collateral_ratio_bps": 20000,
  "liquidation_ratio_bps":    10000,
  "liquidation_bonus_bps":     1000,
  "stability_fee_apy_bps":      200,
  "redemption_fee_bps":          50,
  "min_zusd_mint":          "100000000000000000000"
},

"contracts": {
  "ZUSD": {
    "address": "0x0000000000000000000000000000000000000020",
    "comment":  "ZUSD Stablecoin (pre-deployed)"
  },
  "ZusdVault": {
    "address": "0x0000000000000000000000000000000000000021",
    "comment":  "CDP Vault"
  },
  "ZusdStabilityPool": {
    "address": "0x0000000000000000000000000000000000000022",
    "comment":  "Stability Pool"
  }
},

"allocations": [
  {
    "address":  "0xZUSD_AMM_SEED_ADDRESS",
    "token":    "ZUSD",
    "balance":  "250000000000000000000000",
    "comment":  "250,000 ZUSD for ZBX/ZUSD AMM seed (backed by treasury ZBX)"
  }
]
```