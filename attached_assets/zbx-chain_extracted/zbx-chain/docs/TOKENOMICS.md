# ZBX Tokenomics

**Token**: ZBX (Zebvix Chain native token)
**Total Supply**: 150,000,000 ZBX (hard cap — no inflation after cap)
**Decimals**: 18

---

## Supply Allocation

| Category | Amount | % | Vesting |
|----------|--------|---|---------|
| Block rewards (miners) | 120,010,000 | 80.01% | Over ~100 years via halving |
| Foundation pre-mine | 9,990,000 | 6.66% | 4-year vest, 1-year cliff |
| AMM seed liquidity | 20,000,000 | 13.33% | Locked in genesis pool |
| **Total** | **150,000,000** | **100%** | |

---

## Block Rewards (Emission Schedule)

| Era | Blocks | Block Reward | Duration |
|-----|--------|-------------|---------|
| Era 0 | 0 – 25M | 3 ZBX | ~3.97 years |
| Era 1 | 25M – 50M | 1.5 ZBX | ~3.97 years |
| Era 2 | 50M – 75M | 0.75 ZBX | ~3.97 years |
| Era 3 | 75M – 100M | 0.375 ZBX | ~3.97 years |
| Era N | … | 3 / 2^N ZBX | halving every 25M blocks |

Block time: 5 seconds → 25M blocks ≈ 3.97 years

---

## Fee Mechanism (EIP-1559)

- **Base fee**: Burned (deflationary) — reduces circulating supply
- **Priority fee**: Paid to block producers (validators/sequencers)
- As usage grows → more fees burned → ZBX becomes more scarce

---

## ZUSD Relationship

ZUSD is the native stablecoin backed by ZBX:
- Users lock ZBX → mint ZUSD (max 50% of collateral value)
- ZUSD brings utility demand for ZBX
- Higher ZBX demand → higher price → more ZUSD mintable

---

## Staking

- Validators must stake minimum **100 ZBX** (self-stake)
- Delegators minimum **10 ZBX** per delegation
- Staking rewards: 12-15% APY (from block emissions + fees)
- Unstaking lock: **7 days**

---

## AMM — Native DEX (ZEP-014)

ZBX Chain ships a native Rust AMM (no Solidity) with one canonical pool genesis-deployed at block 1:

| Pool | Fee | Purpose |
|------|-----|---------|
| **ZBX/ZUSD** | 0.30% | Primary ZBX price discovery; ZUSD peg support |

### Genesis Seeding (from AMM allocation)

The 20,000,000 ZBX AMM genesis seed + Foundation ZUSD pre-mint seeds the pool:

| Pool | ZBX side | Stable side | Starting price |
|------|----------|-------------|----------------|
| ZBX/ZUSD | 20,000,000 ZBX | 1,000,000 ZUSD | 1 ZBX ≈ $0.05 |

### AMM Fee Distribution

- LP providers earn **100%** of swap fees (proportional to LP share)
- Fees compound inside the pool (not extracted separately)
- Governance can adjust fee tiers via proposal + 48h timelock

### Security (10-layer checks per swap)

Every swap checks: circuit breaker → reentrancy → deadline → zero-amount →
oracle deviation (≤15%) → price impact (≤30%) → AMM formula → reserve drain (≤30%) →
slippage (min_amount_out) → k-invariant.

---

## Native Stablecoins

| Token | Peg | Genesis Pre-mint | AMM Pool |
|-------|-----|------------------|----------|
| **ZUSD** | 1 USD | 100,000,000 ZUSD → Foundation Treasury | ZBX/ZUSD |

ZUSD is overcollateralized (CDP model, ZEP-002).

---

## Governance

- 1 ZBX (or ZBXGov) = 1 vote
- Proposal threshold: 1,000,000 ZBX (0.67% of supply)
- Quorum: 10% of total supply
- Voting period: 7 days
- Timelock: 48 hours (parameter changes), 7 days (upgrades)