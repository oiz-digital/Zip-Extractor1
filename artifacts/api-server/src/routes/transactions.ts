import { Router, type IRouter } from "express";

const router: IRouter = Router();

const ADDRS = [
  "0xA1b2C3d4E5f6A1b2C3d4E5f6A1b2C3d4E5f6A1b2",
  "0xB2c3D4e5F6a7B2c3D4e5F6a7B2c3D4e5F6a7B2c3",
  "0xC3d4E5f6A7b8C3d4E5f6A7b8C3d4E5f6A7b8C3d4",
  "0xD4e5F6a7B8c9D4e5F6a7B8c9D4e5F6a7B8c9D4e5",
  "0xE5f6A7b8C9d0E5f6A7b8C9d0E5f6A7b8C9d0E5f6",
  "0xF6a7B8c9D0e1F6a7B8c9D0e1F6a7B8c9D0e1F6a7",
];
const TYPES = ["transfer", "contract_call", "contract_creation", "ai_inference", "xcl_transfer", "staking"];

function genHash(): string {
  return "0x" + Array.from({ length: 64 }, () => Math.floor(Math.random() * 16).toString(16)).join("");
}

function genTx(offset = 0) {
  const type = TYPES[offset % TYPES.length];
  return {
    hash: genHash(),
    blockNumber: 4_872_341 - Math.floor(offset / 10),
    timestamp: new Date(Date.now() - offset * 2000).toISOString(),
    from: ADDRS[offset % ADDRS.length],
    to: type === "contract_creation" ? null : ADDRS[(offset + 1) % ADDRS.length],
    status: Math.random() > 0.05 ? "success" : "failed",
    value: (Math.random() * 500).toFixed(6),
    gasPrice: "1200000000",
    gasUsed: (21000 + Math.floor(Math.random() * 200000)).toString(),
    type,
    nonce: offset,
  };
}

router.get("/transactions", async (req, res): Promise<void> => {
  const limit = Math.min(parseInt(req.query.limit as string) || 20, 50);
  const offset = parseInt(req.query.offset as string) || 0;
  const txs = Array.from({ length: limit }, (_, i) => genTx(offset + i));
  res.json(txs);
});

router.get("/transactions/:hash", async (req, res): Promise<void> => {
  const hash = Array.isArray(req.params.hash) ? req.params.hash[0] : req.params.hash;
  if (!hash.startsWith("0x")) {
    res.status(404).json({ error: "Transaction not found" });
    return;
  }
  res.json({
    hash,
    blockNumber: 4_872_341,
    timestamp: new Date().toISOString(),
    from: ADDRS[0],
    to: ADDRS[1],
    status: "success",
    value: "12.500000",
    gasPrice: "1200000000",
    gasUsed: "21000",
    gasLimit: "100000",
    type: "transfer",
    nonce: 42,
    input: "0x",
    logs: [],
    contractAddress: null,
    effectiveGasPrice: "1200000000",
    cumulativeGasUsed: "21000",
  });
});

export default router;
