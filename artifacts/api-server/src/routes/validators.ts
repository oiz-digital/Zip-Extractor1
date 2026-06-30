import { Router, type IRouter } from "express";

const router: IRouter = Router();

const VALIDATORS = [
  { moniker: "Zebvix Labs", website: "https://zebvix.io" },
  { moniker: "Staking Dragon", website: "https://stakingdragon.com" },
  { moniker: "ChainGuard", website: "https://chainguard.io" },
  { moniker: "QuantumNode", website: null },
  { moniker: "ZBX Foundation", website: "https://zbxfoundation.org" },
  { moniker: "NodeRunner Pro", website: null },
  { moniker: "Apex Validators", website: "https://apexval.io" },
  { moniker: "Decentralized Hub", website: null },
  { moniker: "CryptoStake", website: "https://cryptostake.net" },
  { moniker: "Infinity Nodes", website: null },
  { moniker: "Galaxy Validators", website: "https://galaxyval.com" },
  { moniker: "ZeroPoint", website: null },
  { moniker: "Meridian Staking", website: "https://meridianstaking.com" },
  { moniker: "Titan Nodes", website: null },
  { moniker: "SkyBridge Val", website: "https://skybridgeval.io" },
];

function genAddress(i: number): string {
  const hex = i.toString(16).padStart(40, "0");
  return "0x" + hex;
}

function genValidator(i: number, status = "active") {
  const stake = (Math.random() * 5000000 + 500000).toFixed(2);
  const selfStake = (parseFloat(stake) * 0.1).toFixed(2);
  return {
    address: genAddress(i + 1),
    moniker: VALIDATORS[i % VALIDATORS.length].moniker,
    votingPower: stake,
    commission: parseFloat((Math.random() * 10 + 1).toFixed(1)),
    status,
    uptime: parseFloat((Math.random() * 5 + 95).toFixed(2)),
    delegators: Math.floor(Math.random() * 5000) + 100,
    selfStake,
    totalStake: stake,
    rank: i + 1,
    website: VALIDATORS[i % VALIDATORS.length].website,
    description: `Professional validator operating ${VALIDATORS[i % VALIDATORS.length].moniker} with high uptime and security.`,
  };
}

const allValidators = Array.from({ length: 100 }, (_, i) => genValidator(i, i < 90 ? "active" : i < 97 ? "inactive" : "jailed"));

router.get("/validators", async (req, res): Promise<void> => {
  const status = req.query.status as string | undefined;
  let result = allValidators;
  if (status) result = result.filter(v => v.status === status);
  res.json(result);
});

router.get("/validators/stats", async (_req, res): Promise<void> => {
  res.json({
    totalStaked: "450,000,000",
    totalValidators: 100,
    activeValidators: 100,
    bondedRatio: 0.75,
    annualizedReward: 12.4,
    nextEpochIn: Math.floor(Math.random() * 300) + 60,
    slashingEvents: 3,
  });
});

router.get("/validators/:address", async (req, res): Promise<void> => {
  const address = Array.isArray(req.params.address) ? req.params.address[0] : req.params.address;
  const v = allValidators.find(x => x.address.toLowerCase() === address.toLowerCase()) || genValidator(0);
  res.json({
    ...v,
    blocksProposed: Math.floor(Math.random() * 50000) + 10000,
    blocksSignedLast100: Math.floor(Math.random() * 5) + 95,
    recentBlocks: Array.from({ length: 10 }, (_, i) => 4_872_341 - i * 10),
  });
});

export default router;
