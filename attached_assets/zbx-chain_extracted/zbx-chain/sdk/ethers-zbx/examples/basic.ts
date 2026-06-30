/**
 * @zebvix/ethers — Basic usage examples
 *
 * Run: npx ts-node examples/basic.ts
 */
import {
  ZbxProvider,
  ZbxWallet,
  PayId,
  ZUSD,
  ZvmClient,
  zbxMainnet,
} from "@zebvix/ethers";

const RPC = process.env.ZBX_RPC ?? "http://127.0.0.1:8545";

async function main() {
  // ── 1. Connect to ZBX node ───────────────────────────────────────────────
  const provider = new ZbxProvider(RPC);
  console.log("Connected to:", RPC);

  // ── 2. Chain info ─────────────────────────────────────────────────────────
  const info = await provider.zbx.info();
  console.log("Chain:", info.chainName, "| ID:", info.chainId, "| VM:", info.vm);
  console.log("Tip height:", info.tipHeight);

  // ── 3. Standard ethers — unchanged ───────────────────────────────────────
  const blockNumber = await provider.getBlockNumber();
  console.log("Block number (ethers):", blockNumber);

  // ── 4. ZBX price ──────────────────────────────────────────────────────────
  const zvm = new ZvmClient(provider);
  const price = await zvm.zbxPrice();
  console.log("ZBX price:", price, "USD");

  // ── 5. Wallet setup ───────────────────────────────────────────────────────
  const PRIVATE_KEY = process.env.PRIVATE_KEY ?? "0x" + "a".repeat(64);
  const wallet = new ZbxWallet(PRIVATE_KEY, provider);
  console.log("Wallet:", wallet.address);

  // ── 6. Balances ───────────────────────────────────────────────────────────
  const zbxBalance  = await wallet.zbxBalance();
  const zusdBalance = await wallet.zusdBalance();
  console.log("ZBX balance: ", zbxBalance.toString(), "wei");
  console.log("ZUSD balance:", ZUSD.format(zusdBalance), "ZUSD");

  // ── 7. Pay ID resolution ──────────────────────────────────────────────────
  const addr = await PayId.resolve("ali@zbx", provider);
  console.log("ali@zbx →", addr ?? "not found");

  // Validation
  try {
    PayId.validate("invalid-pay-id");
  } catch (e: any) {
    console.log("Validation error (expected):", e.message);
  }

  // Parse
  const parsed = PayId.parse("alice@zbx");
  console.log("Parsed:", parsed); // { name: "alice", handle: "zbx" }

  // ── 8. Pool state ─────────────────────────────────────────────────────────
  const pool = await provider.zbx.pool();
  if (pool.initialized) {
    console.log("Pool ZBX reserve:", pool.zbxReserveWei);
    console.log("Spot price: 1 ZBX =", pool.spotPriceUsdPerZbx, "USD");
  }

  // ── 9. Send ZBX to a Pay ID (auto-resolved) ───────────────────────────────
  // const tx = await wallet.sendZbx("ali@zbx", "10");
  // console.log("Tx hash:", tx.txHash);

  // ── 10. Register a Pay ID ─────────────────────────────────────────────────
  // const reg = await wallet.registerPayId("myname@zbx");
  // console.log("Registered:", reg.payId, "| Tx:", reg.txHash);
}

main().catch(console.error);