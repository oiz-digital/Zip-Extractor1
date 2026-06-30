# ZEP-030: AI Assistant (Phase-1)

| Field     | Value |
|-----------|-------|
| ZEP       | 030 |
| Title     | AI Assistant — Phase 1 (Wallet, Explorer, Contract, Debugger) |
| Author    | Zebvix Core Team |
| Status    | Accepted |
| Category  | AI / Developer Experience |
| Created   | 2026-06-28 |
| Requires  | ZEP-009 (AI Precompile), ZEP-004 (ZVM) |

---

## Abstract

Phase 1 of the Zebvix AI integration delivers four user-facing AI features:

1. **AI Wallet Assistant** — natural-language wallet operations ("send 10 ZBX to alice.zbx")
2. **AI Explorer Search** — semantic search over blocks, transactions, and addresses
3. **AI Contract Explanation** — plain-English explanation of any deployed Solidity contract
4. **AI Error Debugger** — diagnose failed transactions with human-readable explanations

These features use the existing `zbx-ai-precompile` and `zbx-ai-sdk` infrastructure. AI Cloud and AI Marketplace features are explicitly deferred to Phase 2.

---

## Motivation

The EVM is powerful but hostile to non-technical users. A raw transaction hash means nothing; a revert reason `0x08c379a0...` is cryptic. The four features in this ZEP create a usable, friendly layer on top of the raw blockchain for developers and end users alike.

---

## Feature Specifications

### 1. AI Wallet Assistant

Natural-language interface for common wallet operations.

**Supported commands:**
- "Send 10 ZBX to alice.zbx"
- "Stake 1000 ZBX with validator 0x..."
- "Show my transaction history"
- "Bridge 5 ZBX to Ethereum Sepolia"
- "What is my current staking reward?"

**Implementation:**
- Runs client-side in the browser wallet (JS SDK: `zbx.ai.assistant`)
- Parses intent locally with a small instruction-tuned model
- Generates and previews the transaction before user signs
- Falls back to manual mode on ambiguous input

**API:**
```typescript
// zebvix-js/src/ai.ts
class WalletAssistant {
  async parseIntent(input: string): Promise<WalletIntent>;
  async buildTransaction(intent: WalletIntent): Promise<TransactionRequest>;
  async explain(tx: TransactionRequest): Promise<string>;
}
```

---

### 2. AI Explorer Search

Semantic search over the Zebvix block explorer.

**Query examples:**
- "Show all DEX swaps in the last 100 blocks"
- "Find transactions from this address to ZbxAMM"
- "Which validator proposed the most blocks this week?"
- "Show all ZUSD minting events"

**Implementation:**
- Search endpoint: `GET /api/v1/search?q=<natural-language-query>`
- Backend: `crates/zbx-explorer/src/search.rs` — existing module extended
- Model: lightweight intent classifier (query type → GraphQL/SQL query)
- Returns ranked results with relevance scores

**Response format:**
```json
{
  "query": "largest transactions today",
  "result_type": "transactions",
  "results": [...],
  "explanation": "Found 10 transactions over 1,000 ZBX in the last 86,400 seconds."
}
```

---

### 3. AI Contract Explanation

Plain-English explanation of any deployed smart contract.

**URL:** `https://explorer.zebvix.com/contract/0x.../explain`

**Input:** Contract address (explorer fetches bytecode + ABI if verified)

**Output:**
```
This contract is a Uniswap V2-style AMM liquidity pool.
It allows users to:
  • Swap ZBX ↔ ZUSD at market rates
  • Add/remove liquidity and earn 0.3% fees
  • View current reserves and prices

Key functions:
  • swap(amountIn, amountOutMin, path, to, deadline) — Exchange tokens
  • addLiquidity(tokenA, tokenB, ...) — Provide liquidity
  • removeLiquidity(...) — Withdraw liquidity
```

**Implementation:**
- `crates/zbx-explorer/src/ai_explain.rs`
- Uses ABI + bytecode fingerprinting to detect common patterns (ERC-20, AMM, lending, staking)
- Falls back to function signature decoding for unknown contracts

---

### 4. AI Error Debugger

Diagnose failed transactions with human-readable explanations.

**URL:** `https://explorer.zebvix.com/tx/0x.../debug`

**Input:** Transaction hash of a failed transaction

**Output:**
```
❌ Transaction Reverted

Reason: Insufficient balance for transfer
         • Required: 500 ZBX
         • Available: 247.3 ZBX

What happened:
  The transaction attempted to transfer 500 ZBX from your wallet to
  0xRecipient, but your wallet only holds 247.3 ZBX (including gas).

How to fix:
  1. Reduce the transfer amount to ≤ 247 ZBX, OR
  2. Acquire more ZBX (buy on exchange or bridge from another chain)
  3. Ensure you have enough for gas (~0.001 ZBX for this operation)
```

**Implementation:**
- `crates/zbx-trace/src/ai_debug.rs`
- Decodes revert reason from `eth_getTransactionReceipt`
- Traces execution with `debug_traceTransaction`
- Maps known revert reasons to human explanations

---

## Privacy

- AI features process queries locally in the browser where possible.
- No transaction data is sent to external AI APIs.
- All AI inference uses the ZBX AI precompile (on-chain) or local WASM models.

---

## Phase 2 (Not in Scope)

- AI Cloud (inference marketplace)
- AI Marketplace (model registry)
- AI Agent SDK
- AI-powered risk scoring

---

## Reference Implementation

- `sdk/zebvix-js/src/ai.ts` — WalletAssistant
- `crates/zbx-explorer/src/search.rs` — AI search (existing, extended)
- `crates/zbx-explorer/src/ai_explain.rs` — Contract explanation
- `crates/zbx-trace/src/ai_debug.rs` — Error debugger
