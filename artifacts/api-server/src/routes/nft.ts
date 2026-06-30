import { Router, type IRouter } from "express";

const router: IRouter = Router();

function genAddress(i: number): string {
  return "0x" + i.toString(16).padStart(40, "0");
}

const NFT_COLLECTIONS = [
  { name: "Zebvix Genesis", symbol: "ZGN", category: "PFP", totalSupply: 10000, verified: true, description: "The original 10,000-piece genesis collection of Zebvix Chain. Holders get governance boost and validator discounts." },
  { name: "Quantum Keys", symbol: "QKEY", category: "Utility", totalSupply: 5000, verified: true, description: "Post-quantum key NFTs granting access to PQ wallet features and early protocol releases." },
  { name: "ZVM Architects", symbol: "ZVMA", category: "Art", totalSupply: 2000, verified: true, description: "Generative art collection celebrating the ZVM launch. Each piece encodes a unique ZVM opcode sequence." },
  { name: "AI Oracles", symbol: "AIOR", category: "AI", totalSupply: 1000, verified: true, description: "Rare NFTs granting on-chain AI inference credits and priority model access." },
  { name: "DeFi Legends", symbol: "DLEG", category: "Gaming", totalSupply: 8888, verified: false, description: "Play-to-earn gaming characters with on-chain stats stored via ZRC-721G standard." },
  { name: "Cross-Chain Travelers", symbol: "CCT", category: "XCL", totalSupply: 3333, verified: true, description: "Bridge-native NFTs that can exist simultaneously across 8 chains via XCL protocol." },
  { name: "Validator Badges", symbol: "VBDG", category: "Utility", totalSupply: 200, verified: true, description: "Soulbound badges for active validators. Non-transferable proof of contribution." },
];

const GAMING_PROJECTS = [
  { name: "ZBX Legends", category: "Battle RPG", onChain: true, description: "Fully on-chain battle RPG with ZRC-721G characters and item crafting.", features: ["On-chain state", "ZRC-721G items", "PvP tournaments", "AI opponents"] },
  { name: "Crypto Karts", category: "Racing", onChain: true, description: "Real-time racing game with on-chain leaderboards and ZBX prize pools.", features: ["On-chain leaderboards", "ZBX prizes", "NFT karts"] },
  { name: "DeFi Wars", category: "Strategy", onChain: true, description: "Strategy game where players manage on-chain DeFi protocols as in-game factions.", features: ["Real DeFi mechanics", "DAO governance", "Guild system"] },
  { name: "Pixel Galaxy", category: "Metaverse", onChain: false, description: "Metaverse land ownership with on-chain deeds and ZBX in-game economy.", features: ["Land NFTs", "On-chain economy", "Player governance"] },
  { name: "ZBX Chess", category: "Board Games", onChain: true, description: "Provably fair chess with AI opponents powered by AIINFER opcode.", features: ["On-chain moves", "AI via AIINFER", "ZBX stakes"] },
];

router.get("/nft/collections", async (_req, res): Promise<void> => {
  const collections = NFT_COLLECTIONS.map((c, i) => ({
    id: `nft-${i}`,
    ...c,
    floorPrice: (Math.random() * 50 + 0.5).toFixed(3),
    volume24h: (Math.random() * 500000 + 1000).toFixed(2),
    owners: Math.floor(c.totalSupply * (Math.random() * 0.3 + 0.6)),
    contractAddress: genAddress(i + 500),
  }));
  res.json(collections);
});

router.get("/nft/gaming", async (_req, res): Promise<void> => {
  const projects = GAMING_PROJECTS.map((p, i) => ({
    id: `game-${i}`,
    ...p,
    players: Math.floor(Math.random() * 50000) + 1000,
    txLast24h: Math.floor(Math.random() * 100000) + 500,
    totalRevenue: (Math.random() * 10000000 + 100000).toFixed(2),
  }));
  res.json(projects);
});

export default router;
