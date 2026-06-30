import { Router, type IRouter } from "express";

const router: IRouter = Router();

const VALIDATORS = [
  "0xA1b2C3d4E5f6A1b2C3d4E5f6A1b2C3d4E5f6A1b2",
  "0xB2c3D4e5F6a7B2c3D4e5F6a7B2c3D4e5F6a7B2c3",
  "0xC3d4E5f6A7b8C3d4E5f6A7b8C3d4E5f6A7b8C3d4",
  "0xD4e5F6a7B8c9D4e5F6a7B8c9D4e5F6a7B8c9D4e5",
  "0xE5f6A7b8C9d0E5f6A7b8C9d0E5f6A7b8C9d0E5f6",
];

function genHash(): string {
  return "0x" + Array.from({ length: 64 }, () => Math.floor(Math.random() * 16).toString(16)).join("");
}

function genBlock(num: number) {
  const txCount = Math.floor(Math.random() * 200) + 10;
  const gasUsed = (BigInt(txCount) * BigInt(21000) * BigInt(Math.floor(Math.random() * 10) + 1)).toString();
  const gasLimit = "30000000";
  return {
    number: num,
    hash: genHash(),
    timestamp: new Date(Date.now() - (4_872_341 - num) * 5000).toISOString(),
    txCount,
    proposer: VALIDATORS[num % VALIDATORS.length],
    gasUsed,
    gasLimit,
    size: Math.floor(Math.random() * 50000) + 5000,
    parentHash: genHash(),
    stateRoot: genHash(),
  };
}

function genBlockDetail(num: number) {
  const base = genBlock(num);
  const txs = Array.from({ length: Math.min(base.txCount, 20) }, (_, i) => ({
    hash: genHash(),
    blockNumber: num,
    timestamp: base.timestamp,
    from: VALIDATORS[i % VALIDATORS.length],
    to: i % 10 === 0 ? null : VALIDATORS[(i + 1) % VALIDATORS.length],
    status: Math.random() > 0.05 ? "success" : "failed",
    value: (Math.random() * 100).toFixed(6),
    gasPrice: "1200000000",
    gasUsed: "21000",
    type: i % 5 === 0 ? "contract_creation" : i % 3 === 0 ? "contract_call" : "transfer",
    nonce: i,
  }));
  return {
    ...base,
    transactionsRoot: genHash(),
    receiptsRoot: genHash(),
    difficulty: "0",
    totalDifficulty: "0",
    nonce: "0x0000000000000000",
    extraData: "0x5a657276697820426c6f636b",
    baseFeePerGas: "1000000000",
    transactions: txs,
  };
}

router.get("/blocks", async (req, res): Promise<void> => {
  const limit = Math.min(parseInt(req.query.limit as string) || 20, 50);
  const offset = parseInt(req.query.offset as string) || 0;
  const latestBlock = 4_872_341;
  const blocks = Array.from({ length: limit }, (_, i) =>
    genBlock(latestBlock - offset - i)
  );
  res.json(blocks);
});

router.get("/blocks/:blockNumber", async (req, res): Promise<void> => {
  const raw = Array.isArray(req.params.blockNumber) ? req.params.blockNumber[0] : req.params.blockNumber;
  const num = parseInt(raw, 10);
  if (isNaN(num) || num < 0) {
    res.status(404).json({ error: "Block not found" });
    return;
  }
  res.json(genBlockDetail(num));
});

export default router;
