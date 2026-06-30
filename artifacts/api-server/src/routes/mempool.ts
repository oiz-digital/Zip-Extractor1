import { Router, type IRouter } from "express";

const router: IRouter = Router();

const TYPES = ["transfer", "contract_call", "ai_inference", "xcl_transfer", "staking", "unstaking"];

function genHash(): string {
  return "0x" + Array.from({ length: 64 }, () => Math.floor(Math.random() * 16).toString(16)).join("");
}

function genAddress(i: number): string {
  return "0x" + i.toString(16).padStart(40, "0");
}

router.get("/mempool/pending", async (req, res): Promise<void> => {
  const limit = Math.min(parseInt(req.query.limit as string) || 20, 50);
  const txs = Array.from({ length: limit }, (_, i) => ({
    hash: genHash(),
    from: genAddress(i + 1000),
    to: i % 10 === 0 ? null : genAddress(i + 2000),
    nonce: Math.floor(Math.random() * 1000),
    gasPrice: (Math.random() * 5 + 0.8).toFixed(9),
    gasLimit: (21000 + Math.floor(Math.random() * 300000)).toString(),
    value: (Math.random() * 1000).toFixed(6),
    type: TYPES[i % TYPES.length],
    addedAt: new Date(Date.now() - Math.floor(Math.random() * 600000)).toISOString(),
  }));
  res.json(txs);
});

router.get("/mempool/stats", async (_req, res): Promise<void> => {
  res.json({
    pendingCount: Math.floor(Math.random() * 2000) + 500,
    queuedCount: Math.floor(Math.random() * 500) + 50,
    avgGasPrice: "1.24",
    minGasPrice: "0.80",
    maxGasPrice: "12.50",
    oldestTxAge: Math.floor(Math.random() * 600) + 30,
  });
});

export default router;
