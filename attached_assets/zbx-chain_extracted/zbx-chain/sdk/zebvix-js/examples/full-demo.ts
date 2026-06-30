/**
 * zebvix.js — Full advanced demo
 *
 * Showcases all major features: subscriptions, batch, contract, fee, AMM, AA
 */
import {
  ZbxClient,
  ZbxWallet,
  ZbxSubscriber,
  ZbxBatch,
  ZbxContract,
  AmmHelper,
  AaHelper,
  logger,
  cache,
  retry,
  ZbxPayIdNotFoundError,
  ZBX,
} from "zebvix.js";

// SECURITY: Always use https:// (RPC) and wss:// (WebSocket) in production.
// The env vars below should point to your TLS-terminated endpoints.
// Local dev defaults (http/ws) are intentional for localhost-only usage.
const RPC = process.env.ZBX_RPC ?? "http://127.0.0.1:8545";  // prod: https://rpc.zbx.io
const WS  = process.env.ZBX_WS  ?? "ws://127.0.0.1:8546";   // prod: wss://ws.zbx.io

async function main() {
  // ── 1. Client with middleware ─────────────────────────────────────────────
  const zbx = new ZbxClient(RPC);
  zbx.use(logger());          // log all calls
  zbx.use(cache(10_000));     // cache reads for 10s
  zbx.use(retry(3));          // retry 3x on failure

  // ── 2. Batch RPC — load everything in 1 HTTP request ─────────────────────
  const batch = new ZbxBatch(zbx);
  const blockP  = batch.blockNumber();
  const priceP  = batch.price();
  const aliP    = batch.resolvePayId("ali@zbx");
  await batch.execute();  // ONE HTTP request

  console.log("Block:", await blockP);
  console.log("Price:", (await priceP).zbxUsd, "USD");
  console.log("ali@zbx:", await aliP);

  // ── 3. WebSocket subscriptions ────────────────────────────────────────────
  const sub = new ZbxSubscriber(WS);
  console.log("Subscribing to events...");

  const unsubBlock = sub.onBlock(block => {
    console.log("NEW BLOCK #" + block.height, block.hash.slice(0, 16) + "...");
  });

  sub.onPayIdRegister(info => {
    console.log("NEW PAY ID:", info.payId, "→", info.address);
  });

  sub.onBurn(event => {
    console.log("ZBX BURNED:", event.amountZbx, "ZBX from", event.from);
  });

  // Stop after 30 seconds
  setTimeout(() => {
    unsubBlock();
    sub.unsubscribe();
    console.log("Unsubscribed from WebSocket");
  }, 30_000);

  // ── 4. AMM pool operations ────────────────────────────────────────────────
  const amm  = new AmmHelper(zbx);
  const pool = await amm.pool();

  if (pool.initialized) {
    console.log("\\nAMM Pool:");
    console.log("  ZBX reserve:", pool.zbxReserveZbx, "ZBX");
    console.log("  ZUSD reserve:", pool.zusdReserve, "ZUSD");
    console.log("  Spot price: 1 ZBX =", pool.spotPriceUsd, "USD");
    console.log("  TVL:", pool.tvlUsd, "USD");

    // Quote: 100 ZBX → how much ZUSD?
    const quote = amm.quoteZbxIn("100", pool);
    console.log("\\nSwap quote: 100 ZBX →", quote.amountOut, "ZUSD");
    console.log("  Price impact:", quote.priceImpact + "%");
    console.log("  Fee:", quote.fee, "ZBX");

    // Reverse: 10000 ZUSD → how much ZBX?
    const rquote = amm.quoteZusdIn("10000", pool);
    console.log("Swap quote: 10000 ZUSD →", rquote.amountOut, "ZBX");

    // Liquidity quote
    const lq = amm.quoteLiquidity("1000", pool);
    console.log("\\nLiquidity (1000 ZBX):");
    console.log("  Need:", lq.zusdIn, "ZUSD");
    console.log("  Get:", lq.lpOut, "LP tokens");
    console.log("  Pool share:", lq.share + "%");
  }

  // ── 5. Contract interaction ───────────────────────────────────────────────
  const ERC20_ABI = [
    { type: "function", name: "name",     inputs: [], outputs: [{ name: "", type: "string" }], stateMutability: "view" },
    { type: "function", name: "symbol",   inputs: [], outputs: [{ name: "", type: "string" }], stateMutability: "view" },
    { type: "function", name: "decimals", inputs: [], outputs: [{ name: "", type: "uint8"  }], stateMutability: "view" },
    { type: "function", name: "balanceOf", inputs: [{ name: "account", type: "address" }], outputs: [{ name: "", type: "uint256" }], stateMutability: "view" },
    { type: "function", name: "transfer", inputs: [{ name: "to", type: "address" }, { name: "amount", type: "uint256" }], outputs: [{ name: "", type: "bool" }], stateMutability: "nonpayable" },
  ] as const;

  // const token    = new ZbxContract(zbx, "0xTokenAddress...", ERC20_ABI);
  // const name     = await token.call<string>("name");
  // const balance  = await token.call<bigint>("balanceOf", ["0x742d35..."]);
  // console.log("Token:", name, "| Balance:", balance.toString());

  // ── 6. Account Abstraction ────────────────────────────────────────────────
  // const aa = new AaHelper(zbx);
  //
  // // Single gasless transfer
  // const op = await aa.buildTransfer({
  //   sender: "0xSmartWallet...",
  //   to:     "ali@zbx",
  //   value:  "100",
  //   paymaster: "0xPaymaster...",  // sponsor gas
  // });
  //
  // const hash = await aa.submit(op);
  // const receipt = await aa.waitForUserOp(hash);
  // console.log("UserOp:", receipt.success ? "SUCCESS" : "FAILED");
  //
  // // Batch: send to multiple Pay IDs in 1 tx
  // const batchOp = await aa.buildBatch({
  //   sender: "0xSmartWallet...",
  //   calls: [
  //     { to: "ali@zbx",   value: "10" },
  //     { to: "bob@zbx",   value: "20" },
  //     { to: "carol@zbx", value: "30" },
  //   ],
  // });
  // await aa.submit(batchOp);

  // ── 7. Pay ID error handling ──────────────────────────────────────────────
  try {
    const PRIVATE_KEY = "0x" + "a".repeat(64);
    const wallet = zbx.wallet(PRIVATE_KEY);
    await wallet.send("doesnotexist@zbx", "1");
  } catch (err) {
    if (err instanceof ZbxPayIdNotFoundError) {
      console.log("Caught typed error:", err.name, "→", err.payId);
    }
  }

  // ── 8. Fee estimation ─────────────────────────────────────────────────────
  const fromAddr = "0x742d35Cc6634C0532925a3b844Bc454e4438f44e";
  const toAddr   = "0x000000000000000000000000000000000000dEaD";
  const transferFee = await zbx.fee.estimateTransfer(fromAddr, toAddr, "100");
  console.log("\\nFee estimate (100 ZBX transfer):");
  console.log("  Gas limit:", transferFee.gasLimit.toString());
  console.log("  Gas price:", transferFee.gasPrice.toString(), "wei");
  console.log("  Fee:", transferFee.feeZbx, "ZBX");

  const payIdFee = await zbx.fee.estimatePayIdRegister(fromAddr);
  console.log("Fee estimate (Pay ID registration):");
  console.log("  Total cost:", payIdFee.totalZbx, "ZBX (1 ZBX + gas)");
}

main().catch(console.error);