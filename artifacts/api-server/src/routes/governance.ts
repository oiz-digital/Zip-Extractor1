import { Router, type IRouter } from "express";

const router: IRouter = Router();

const PROPOSALS = [
  {
    id: 45, zepNumber: 45, title: "ZEP-045: Quantum-Resistant Key Migration Phase 3", type: "Protocol Upgrade",
    status: "active", summary: "Complete the third phase of post-quantum key migration to Dilithium-3 signatures for all validators.",
    description: "This proposal completes the migration of validator signing keys to NIST-standardized Dilithium-3 post-quantum signatures as specified in ZEP-015. Phase 3 covers the remaining 15% of validators who have not yet migrated their keys.\n\nThe migration window will last 14 days from proposal passing, after which validators still using legacy ECDSA keys will be temporarily suspended from consensus participation.",
    changes: ["Mandate Dilithium-3 keys for all active validators", "Implement 14-day migration window", "Auto-suspend non-compliant validators after deadline"],
    endTime: new Date(Date.now() + 7 * 24 * 3600000).toISOString(),
    yesVotes: "62,400,000", noVotes: "8,100,000", abstainVotes: "3,200,000",
    proposer: "0xA1b2C3d4E5f6A1b2C3d4E5f6A1b2C3d4E5f6A1b2",
  },
  {
    id: 44, zepNumber: 44, title: "ZEP-044: Increase Block Gas Limit to 60M", type: "Parameter Change",
    status: "passed", summary: "Increase the block gas limit from 30M to 60M to support higher throughput.",
    description: "As network usage has grown significantly over the past quarter, this proposal increases the block gas limit from 30,000,000 to 60,000,000 gas units. Validator hardware requirements have been benchmarked to handle this increase without degrading consensus latency.",
    changes: ["Increase block gas limit from 30M to 60M", "Update gas estimation recommendations"],
    endTime: new Date(Date.now() - 2 * 24 * 3600000).toISOString(),
    yesVotes: "78,200,000", noVotes: "2,800,000", abstainVotes: "1,100,000",
    proposer: "0xB2c3D4e5F6a7B2c3D4e5F6a7B2c3D4e5F6a7B2c3",
  },
  {
    id: 43, zepNumber: 43, title: "ZEP-043: Native Gaming Integration (ZEP-031 Activation)", type: "Feature Activation",
    status: "passed", summary: "Activate on-chain gaming precompiles and ZRC-721G standard for gaming NFTs.",
    description: "Activates the gaming-specific precompiles defined in ZEP-031, enabling low-latency state transitions for on-chain games. Introduces ZRC-721G, an extension of ZRC-721 with batch minting and game-state embedding.",
    changes: ["Activate gaming precompiles at 0xC8-0xCA", "Deploy ZRC-721G standard library", "Enable in-game asset trading on native AMM"],
    endTime: new Date(Date.now() - 14 * 24 * 3600000).toISOString(),
    yesVotes: "71,500,000", noVotes: "4,200,000", abstainVotes: "2,800,000",
    proposer: "0xC3d4E5f6A7b8C3d4E5f6A7b8C3d4E5f6A7b8C3d4",
  },
  {
    id: 42, zepNumber: 42, title: "ZEP-042: Reduce Validator Commission Cap to 20%", type: "Parameter Change",
    status: "rejected", summary: "Reduce maximum validator commission from 30% to 20% to protect delegators.",
    description: "This proposal aimed to cap validator commission at 20% to improve delegator returns and encourage fairer validator economics. After community discussion, concerns about validator profitability in a bear market led to rejection.",
    changes: ["Reduce max commission from 30% to 20%", "Grace period of 30 days for existing validators to adjust"],
    endTime: new Date(Date.now() - 21 * 24 * 3600000).toISOString(),
    yesVotes: "28,100,000", noVotes: "55,400,000", abstainVotes: "8,200,000",
    proposer: "0xD4e5F6a7B8c9D4e5F6a7B8c9D4e5F6a7B8c9D4e5",
  },
  {
    id: 46, zepNumber: 46, title: "ZEP-046: XCL Expansion — Solana & Cosmos Support", type: "Protocol Upgrade",
    status: "pending", summary: "Add Solana and Cosmos IBC as supported chains in the Native Cross-Chain Layer.",
    description: "Extends ZEP-026 (XCL) to support Solana via SPL token bridging and Cosmos IBC protocol. This would bring the total supported chains to 11, covering 85% of DeFi TVL.",
    changes: ["Add Solana SPL bridge adapter", "Implement Cosmos IBC relayer", "Update XCL smart contracts"],
    endTime: new Date(Date.now() + 14 * 24 * 3600000).toISOString(),
    yesVotes: "0", noVotes: "0", abstainVotes: "0",
    proposer: "0xE5f6A7b8C9d0E5f6A7b8C9d0E5f6A7b8C9d0E5f6",
  },
];

router.get("/governance/proposals", async (req, res): Promise<void> => {
  const status = req.query.status as string | undefined;
  let result = PROPOSALS;
  if (status) result = result.filter(p => p.status === status);
  res.json(result.map(({ description, changes, ...p }) => ({ ...p, voters: undefined })));
});

router.get("/governance/proposals/:id", async (req, res): Promise<void> => {
  const raw = Array.isArray(req.params.id) ? req.params.id[0] : req.params.id;
  const id = parseInt(raw, 10);
  const proposal = PROPOSALS.find(p => p.id === id);
  if (!proposal) {
    res.status(404).json({ error: "Proposal not found" });
    return;
  }
  res.json({ ...proposal, voters: [] });
});

export default router;
