import { Router, type IRouter } from "express";

const router: IRouter = Router();

const PRICE_FEEDS = [
  { pair: "ZBX/USD", base: 9.46 },
  { pair: "BTC/USD", base: 67420 },
  { pair: "ETH/USD", base: 3521 },
  { pair: "BNB/USD", base: 598 },
  { pair: "SOL/USD", base: 168 },
  { pair: "MATIC/USD", base: 0.88 },
  { pair: "ARB/USD", base: 1.24 },
  { pair: "OP/USD", base: 2.87 },
  { pair: "AVAX/USD", base: 42.3 },
  { pair: "LINK/USD", base: 18.7 },
  { pair: "UNI/USD", base: 12.4 },
  { pair: "AAVE/USD", base: 186 },
  { pair: "MKR/USD", base: 2841 },
  { pair: "ZUSD/USD", base: 1.0 },
];

router.get("/oracle/prices", async (_req, res): Promise<void> => {
  const prices = PRICE_FEEDS.map(f => {
    const change24h = f.pair === "ZUSD/USD" ? 0 : parseFloat((Math.random() * 12 - 6).toFixed(2));
    const price = f.base * (1 + change24h / 100);
    const high24h = price * (1 + Math.abs(change24h / 100) + 0.01);
    const low24h = price * (1 - Math.abs(change24h / 100) - 0.01);
    return {
      pair: f.pair,
      price: price.toFixed(f.base < 1 ? 4 : f.base < 100 ? 3 : 2),
      change24h,
      high24h: high24h.toFixed(f.base < 1 ? 4 : f.base < 100 ? 3 : 2),
      low24h: low24h.toFixed(f.base < 1 ? 4 : f.base < 100 ? 3 : 2),
      volume24h: (Math.random() * 1000000000 + 10000000).toFixed(2),
      sources: Math.floor(Math.random() * 4) + 5,
      lastUpdated: new Date().toISOString(),
      deviation: parseFloat((Math.random() * 0.3).toFixed(3)),
    };
  });
  res.json(prices);
});

export default router;
