import { Router, type IRouter } from "express";

const router: IRouter = Router();

const CHAINS = ["Ethereum", "Polygon", "Arbitrum", "Optimism", "BNB Chain", "Avalanche", "Solana", "Base"];
const ASSETS = ["ZBX", "ETH", "USDC", "USDT", "WBTC", "BNB", "ZUSD"];
const STATUSES = ["pending", "relayed", "finalized", "failed"] as const;
const PROOF_TYPES = ["BLS Light Client", "MPT State Proof", "ZK Proof", "BLS Aggregate"];

function genHash(): string {
  return "0x" + Array.from({ length: 64 }, () => Math.floor(Math.random() * 16).toString(16)).join("");
}

function genAddress(i: number): string {
  return "0x" + i.toString(16).padStart(40, "0");
}

router.get("/xcl/transfers", async (req, res): Promise<void> => {
  const limit = Math.min(parseInt(req.query.limit as string) || 20, 50);
  const filterChain = req.query.chain as string | undefined;
  const transfers = Array.from({ length: limit }, (_, i) => {
    const srcChain = filterChain || CHAINS[i % CHAINS.length];
    const destChain = srcChain === "Zebvix" ? CHAINS[(i + 1) % CHAINS.length] : "Zebvix";
    const weights = [0.15, 0.60, 0.20, 0.05];
    const rand = Math.random();
    let status: string = "finalized";
    let cum = 0;
    for (let j = 0; j < weights.length; j++) {
      cum += weights[j];
      if (rand < cum) { status = STATUSES[j]; break; }
    }
    return {
      id: `xcl-${i + 1}`,
      txHash: genHash(),
      sourceChain: srcChain,
      destChain,
      asset: ASSETS[i % ASSETS.length],
      amount: (Math.random() * 10000 + 10).toFixed(4),
      status,
      timestamp: new Date(Date.now() - i * 60000).toISOString(),
      sender: genAddress(i + 100),
      receiver: genAddress(i + 200),
      proofType: PROOF_TYPES[i % PROOF_TYPES.length],
    };
  });
  res.json(transfers);
});

router.get("/xcl/stats", async (_req, res): Promise<void> => {
  res.json({
    totalTransfers: 1_482_771,
    volume24h: "142,500,000",
    supportedChains: CHAINS.length + 1,
    activeRelayers: 24,
    avgFinalizationTime: 8.4,
  });
});

export default router;
