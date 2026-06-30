import { Router, type IRouter } from "express";

const router: IRouter = Router();

const POOLS = [
  { token0: "ZBX", token1: "ZUSD", fee: 0.3 },
  { token0: "ZBX", token1: "ETH", fee: 0.3 },
  { token0: "ZUSD", token1: "USDC", fee: 0.05 },
  { token0: "ETH", token1: "ZUSD", fee: 0.3 },
  { token0: "ZBX", token1: "BTC", fee: 1.0 },
  { token0: "WBTC", token1: "ZUSD", fee: 0.3 },
  { token0: "ZBX", token1: "USDC", fee: 0.3 },
];

const LENDING_ASSETS = [
  { asset: "ZBX", collateralFactor: 0.7, liquidationThreshold: 0.8 },
  { asset: "ZUSD", collateralFactor: 0.9, liquidationThreshold: 0.95 },
  { asset: "ETH", collateralFactor: 0.75, liquidationThreshold: 0.85 },
  { asset: "BTC", collateralFactor: 0.7, liquidationThreshold: 0.82 },
  { asset: "USDC", collateralFactor: 0.9, liquidationThreshold: 0.95 },
];

const PERP_SYMBOLS = ["BTC-PERP", "ETH-PERP", "ZBX-PERP", "SOL-PERP", "BNB-PERP", "ARB-PERP"];
const PERP_PRICES: Record<string, number> = {
  "BTC-PERP": 67420, "ETH-PERP": 3521, "ZBX-PERP": 9.46,
  "SOL-PERP": 168, "BNB-PERP": 598, "ARB-PERP": 1.24,
};

router.get("/defi/pools", async (_req, res): Promise<void> => {
  const pools = POOLS.map((p, i) => {
    const tvl = (Math.random() * 50000000 + 1000000).toFixed(2);
    return {
      id: `pool-${i}`,
      ...p,
      tvl,
      volume24h: (parseFloat(tvl) * (Math.random() * 0.3 + 0.05)).toFixed(2),
      apy: parseFloat((Math.random() * 40 + 5).toFixed(1)),
      txCount: Math.floor(Math.random() * 10000) + 500,
      token0Reserve: (Math.random() * 1000000 + 100000).toFixed(4),
      token1Reserve: (Math.random() * 1000000 + 100000).toFixed(4),
      priceToken0: (Math.random() * 100 + 1).toFixed(4),
      priceToken1: (Math.random() * 100 + 1).toFixed(4),
    };
  });
  res.json(pools);
});

router.get("/defi/lending", async (_req, res): Promise<void> => {
  const markets = LENDING_ASSETS.map(a => {
    const totalSupply = (Math.random() * 100000000 + 5000000).toFixed(2);
    const utilization = parseFloat((Math.random() * 0.5 + 0.3).toFixed(2));
    const totalBorrow = (parseFloat(totalSupply) * utilization).toFixed(2);
    return {
      ...a,
      totalSupply,
      totalBorrow,
      supplyApy: parseFloat((utilization * 10 + 1).toFixed(2)),
      borrowApy: parseFloat((utilization * 15 + 2).toFixed(2)),
      utilization,
      price: (Math.random() * 1000 + 1).toFixed(2),
    };
  });
  res.json(markets);
});

router.get("/defi/perps", async (_req, res): Promise<void> => {
  const markets = PERP_SYMBOLS.map(symbol => {
    const basePrice = PERP_PRICES[symbol] || 100;
    const change24h = parseFloat((Math.random() * 10 - 5).toFixed(2));
    return {
      symbol,
      price: (basePrice * (1 + change24h / 100)).toFixed(2),
      change24h,
      volume24h: (Math.random() * 500000000 + 10000000).toFixed(2),
      openInterest: (Math.random() * 200000000 + 5000000).toFixed(2),
      fundingRate: parseFloat((Math.random() * 0.02 - 0.01).toFixed(4)),
      maxLeverage: [10, 20, 50, 100][Math.floor(Math.random() * 4)],
      longShortRatio: parseFloat((Math.random() * 0.6 + 0.4).toFixed(2)),
    };
  });
  res.json(markets);
});

router.get("/defi/stats", async (_req, res): Promise<void> => {
  res.json({
    totalTvl: "1,240,000,000",
    totalVolume24h: "87,500,000",
    totalProtocols: 24,
    ammTvl: "620,000,000",
    lendingTvl: "480,000,000",
    perpVolume24h: "52,300,000",
    yieldTvl: "140,000,000",
  });
});

export default router;
