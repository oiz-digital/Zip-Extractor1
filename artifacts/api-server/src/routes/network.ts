import { Router, type IRouter } from "express";

const router: IRouter = Router();

/* Mainnet counters */
let mainnetBlockHeight = 4_872_341;
let mainnetTotalTxs = 142_887_654;

/* Testnet counters */
let testnetBlockHeight = 1_284_507;
let testnetTotalTxs = 8_234_112;

setInterval(() => {
  mainnetBlockHeight += 1;
  mainnetTotalTxs += Math.floor(Math.random() * 30) + 5;
  testnetBlockHeight += 1;
  testnetTotalTxs += Math.floor(Math.random() * 8) + 1;
}, 5000);

function getNetwork(req: any): "mainnet" | "testnet" {
  const n = (req.headers["x-zbx-network"] || req.query.network || "mainnet") as string;
  return n === "testnet" ? "testnet" : "mainnet";
}

router.get("/network/stats", async (req, res): Promise<void> => {
  const net = getNetwork(req);
  if (net === "testnet") {
    res.json({
      blockHeight: testnetBlockHeight,
      totalTxs: testnetTotalTxs,
      totalAddresses: 45_823,
      totalValidators: 32,
      marketCap: "0",
      circulatingSupply: "500,000,000",
      totalSupply: "1,000,000,000",
      chainId: 8990,
      networkName: "Zebvix Testnet",
      avgBlockTime: 5.0,
      avgGasPrice: "0.1",
    });
  } else {
    res.json({
      blockHeight: mainnetBlockHeight,
      totalTxs: mainnetTotalTxs,
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
  }
});

router.get("/network/overview", async (req, res): Promise<void> => {
  const net = getNetwork(req);
  if (net === "testnet") {
    res.json({
      tps: parseFloat((Math.random() * 80 + 40).toFixed(1)),
      blockTime: 5.0,
      finalityTime: 5.0,
      activeValidators: 28,
      epochNumber: 1104,
      epochProgress: Math.random() * 100,
      peerCount: Math.floor(Math.random() * 20) + 15,
      uptime: 98.12,
      consensusRound: Math.floor(Math.random() * 3) + 1,
    });
  } else {
    res.json({
      tps: parseFloat((Math.random() * 800 + 200).toFixed(1)),
      blockTime: 5.0,
      finalityTime: 5.0,
      activeValidators: 100,
      epochNumber: 4821,
      epochProgress: Math.random() * 100,
      peerCount: Math.floor(Math.random() * 50) + 200,
      uptime: 99.97,
      consensusRound: Math.floor(Math.random() * 5) + 1,
    });
  }
});

export default router;
