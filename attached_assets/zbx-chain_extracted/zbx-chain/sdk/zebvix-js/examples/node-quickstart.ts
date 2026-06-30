/**
 * zebvix.js — Node.js quick start
 *
 * Run: npx ts-node examples/node-quickstart.ts
 * Or:  ZBX_RPC=http://127.0.0.1:8545 npx ts-node examples/node-quickstart.ts
 */
import { ZbxClient, ZBX } from "zebvix.js";

const rpc = process.env.ZBX_RPC ?? ZBX.MAINNET_RPC;

async function main() {
  // ── 1. Connect ────────────────────────────────────────────────────────────
  const zbx = new ZbxClient(rpc);
  console.log("zebvix.js", { rpc });

  // ── 2. Chain info ─────────────────────────────────────────────────────────
  const info = await zbx.getChainInfo();
  console.log("Chain:", info.chainName, "| ID:", info.chainId, "| VM:", info.vm);
  console.log("Block #:", info.tipHeight, "| Block time:", info.blockTimeSecs, "sec");

  // ── 3. ZVM info ───────────────────────────────────────────────────────────
  console.log("ZVM version:", zbx.zvm.version);
  console.log("ZVM block time:", zbx.zvm.blockTimeMs, "ms");

  // ── 4. Price ──────────────────────────────────────────────────────────────
  const price = await zbx.zvm.price();
  console.log("ZBX price:", price, "USD");

  // ── 5. Balance ────────────────────────────────────────────────────────────
  const addr = "0x742d35Cc6634C0532925a3b844Bc454e4438f44e";
  const balWei = await zbx.getBalance(addr);
  console.log("Balance (wei):", balWei.toString());
  console.log("Balance (ZBX):", zbx.formatZbx(balWei));

  const zusdBal = await zbx.zusd.balanceOf(addr);
  console.log("ZUSD balance:", zbx.zusd.format(zusdBal), "ZUSD");

  // ── 6. Pay ID ─────────────────────────────────────────────────────────────
  const payIdAddr = await zbx.payId.resolve("ali@zbx");
  console.log("ali@zbx →", payIdAddr ?? "not found");

  const available = await zbx.payId.isAvailable("newname@zbx");
  console.log("newname@zbx available:", available);

  zbx.payId.validate("ali@zbx");  // no error
  console.log("Pay ID parse:", zbx.payId.parse("ali@zbx")); // {name: "ali", handle: "zbx"}

  // ── 7. ZVM disassemble ────────────────────────────────────────────────────
  const opcodes = zbx.zvm.disassemble("6003600401c200");
  console.log("Disasm:", opcodes);
  // ["0000  60  PUSH1  0x03", "0002  60  PUSH1  0x04", "0004  01  ADD", "0005  c2  ZBXPRICE  ← ZVM"]

  // ── 8. Wallet ─────────────────────────────────────────────────────────────
  // const wallet = zbx.wallet(process.env.PRIVATE_KEY!);
  // const tx = await wallet.send("ali@zbx", "10");
  // console.log("Sent:", tx.hash);
}

main().catch(console.error);