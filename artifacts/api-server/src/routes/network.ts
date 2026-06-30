import { Router, type IRouter } from "express";
import { logger } from "../lib/logger";

const router: IRouter = Router();

let blockHeight = 4_872_341;
let totalTxs = 142_887_654;

setInterval(() => {
  blockHeight += 1;
  totalTxs += Math.floor(Math.random() * 30) + 5;
}, 5000);

router.get("/network/stats", async (_req, res): Promise<void> => {
  res.json({
    blockHeight,
    totalTxs,
    totalAddresses: 1_234_567,
    totalValidators: 128,
    marketCap: "1,420,000,000",
    circulatingSupply: "89,400,000",
    totalSupply: "150,000,000",
    chainId: 8989,
    networkName: "Zebvix Mainnet",
    avgBlockTime: 5.0,
    avgGasPrice: "1.2",
  });
});

router.get("/network/overview", async (_req, res): Promise<void> => {
  const tps = (Math.random() * 800 + 200).toFixed(1);
  res.json({
    tps: parseFloat(tps),
    blockTime: 5.0,
    finalityTime: 5.0,
    activeValidators: 100,
    epochNumber: 4821,
    epochProgress: Math.random() * 100,
    peerCount: Math.floor(Math.random() * 50) + 200,
    uptime: 99.97,
    consensusRound: Math.floor(Math.random() * 5) + 1,
  });
});

export default router;
