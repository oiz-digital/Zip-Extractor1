# Zebvix Smart Contracts

Solidity contracts that live on **external EVM chains** (BNB Chain, Ethereum,
Polygon, …) and on the **Zebvix EVM layer** (Phase C).

## Layout

```
contracts/
├── ZRC20.sol          BEP-20 / ERC-20 ZBX token (deploys to BSC)
├── BridgeVault.sol        Lock / release vault (deploys to BSC)
├── BridgeMultisig.sol     N-of-M oracle multisig (deploys to BSC)
├── interfaces/
│   ├── IBridgeVault.sol   Public interface for relayers + dApps
│   └── IZBX.sol           Minimal ERC-20 + bridge-mint extension
└── README.md              (this file)
```

## Bridge architecture (Phase B.13 wiring)

```
   ┌─────────────────────┐                       ┌─────────────────────┐
   │   Zebvix L1 (8989)  │                       │     BNB Chain (56)  │
   │                     │                       │                     │
   │  user.bridge_out  ──┼──► BridgeOutEvent  ──►│  Oracle.execute()   │
   │                     │   (off-chain relay)   │   ├── ZBX.mint()    │
   │  vault locks ZBX    │                       │   └── to recipient  │
   │                     │                       │                     │
   │  admin.bridge_in  ◄─┼──── BridgeOut(ZBX) ◄──┤  user → Vault.lock()│
   │   (replay-prot.)    │   (off-chain relay)   │                     │
   │  vault releases ZBX │                       │  ZBX.burn(amount)   │
   └─────────────────────┘                       └─────────────────────┘
```

### Outbound (Zebvix → BSC)
1. User signs `TxKind::Bridge(BridgeOut)` on Zebvix.
2. Chain debits user's ZBX, credits internal vault `0x7a627…0000`.
3. Chain emits sequenced `BridgeOutEvent { seq, asset_id, dest, amount }`.
4. Off-chain relayer polls `zbx_recentBridgeOutEvents`.
5. Relayer signs `BridgeMultisig.executeMint(seq, dest, amount)` on BSC.
6. Once threshold reached, `ZRC20.mint(dest, amount)` is called.
7. User now holds wrapped ZBX on BSC.

### Inbound (BSC → Zebvix)
1. User calls `BridgeVault.lock(amount, zebvix_dest)` on BSC.
2. Vault transfers BEP-20 ZBX from user → contract; emits `Locked` event.
3. Off-chain relayer picks up `Locked`, computes `source_tx_hash`.
4. Relayer signs `Zebvix.bridge_in(asset_id, source_tx_hash, recipient, amount)`.
5. Zebvix verifies replay protection, debits internal vault, credits recipient.

### Security model
- **Lock/release, not burn/mint on outbound:** total ZBX supply on BSC ≤
  total ZBX locked on Zebvix; bridge vault is auditable on-chain.
- **Replay protection:** every `BridgeIn` claim is keyed by
  `(asset_id, source_tx_hash)` — second submission is rejected.
- **N-of-M multisig oracle (B.13):** mint authority on BSC requires M
  signatures from the relayer set. Single-key oracle (B.12 current) is for
  dev/testnet only.
- **Pause switch:** founder can pause both vault + token mint via
  `BridgeMultisig.pause()` for emergency response.

## Deployment

### Prerequisites
- Hardhat 2.22+ or Foundry 0.2.0+
- BSC testnet RPC (`https://data-seed-prebsc-1-s1.binance.org:8545`) for
  staging; BSC mainnet for production
- BSC deployer wallet with ~0.05 BNB for gas
- 5 oracle relayer addresses (for BridgeMultisig.deploy())

### Order (post-architect-review, deadlock-free)

The constructor cycle Multisig ↔ Vault ↔ Token is broken using a
**set-once-then-lock** pattern. `vault` on both `BridgeMultisig` and
`ZRC20` is mutable until `lockVault()` is called by the founder, after
which it is permanently fixed.

1. Deploy `BridgeMultisig(initialRelayers, threshold, founder)`. Vault is
   left unset (`address(0)`) — `submitMint` will revert until it is set.
2. Deploy `ZRC20(founder)`. Vault is left unset — bridge-mint/burn will
   revert until it is set.
3. Deploy `BridgeVault(token=ZRC20.address, multisig=BridgeMultisig.address, founder)`.
   Both peer addresses are real and immutable on this side.
4. Founder tx: `BridgeMultisig.setVault(BridgeVault.address)`.
5. Founder tx: `ZRC20.setVault(BridgeVault.address)`.
6. Verify with a small test mint (e.g. submit 1 wei via the relayer key on a
   BSC testnet fork). Confirm `Locked` + `Minted` event flow.
7. Founder tx: `BridgeMultisig.lockVault()` — vault address becomes permanent.
8. Founder tx: `ZRC20.lockVault()` — token's bridge minter becomes permanent.
9. Update Zebvix `bridge-register-network 56` if not already done.
10. Update Zebvix `bridge-register-asset` with `--contract <ZRC20.addr>`.

> **Important:** steps 7 and 8 are irreversible. Do them only after you've
> tested an end-to-end mint + lock round-trip on testnet.

### Sample Hardhat config
```js
module.exports = {
  solidity: "0.8.24",
  networks: {
    bscTestnet: {
      url: "https://data-seed-prebsc-1-s1.binance.org:8545",
      chainId: 97,
      accounts: [process.env.BSC_DEPLOYER_KEY],
    },
    zebvix: {
      url: "http://93.127.213.192:8545",
      chainId: 8989,
      accounts: [process.env.ZEBVIX_FOUNDER_KEY],
    },
  },
};
```

## Audit checklist

- [ ] Reentrancy protection on `BridgeVault.lock()` and `release()`
- [ ] Integer overflow protection (Solidity 0.8+ built-in checked math)
- [ ] Access control: only `BridgeMultisig` can mint/burn ZRC20
- [ ] Replay protection: nonce / source_tx_hash uniqueness
- [ ] Pause + emergency-recovery flows
- [ ] No `tx.origin` usage; `msg.sender` everywhere
- [ ] Events emitted for every state-changing call
- [ ] Constructor args validated (no zero-address for critical roles)
- [ ] Test coverage ≥ 95% (Hardhat coverage / forge coverage)

## License

MIT — same as the rest of Zebvix.