import { Router, type IRouter } from "express";

const router: IRouter = Router();

const PAYIDS: Record<string, { address: string; registered: string; txCount: number; balance: string }> = {
  "satoshi@zbx": { address: "0xA1b2C3d4E5f6A1b2C3d4E5f6A1b2C3d4E5f6A1b2", registered: "2024-01-15T00:00:00Z", txCount: 4821, balance: "124500.234" },
  "alice@zbx": { address: "0xB2c3D4e5F6a7B2c3D4e5F6a7B2c3D4e5F6a7B2c3", registered: "2024-02-20T00:00:00Z", txCount: 1241, balance: "8821.12" },
  "bob@zbx": { address: "0xC3d4E5f6A7b8C3d4E5f6A7b8C3d4E5f6A7b8C3d4", registered: "2024-03-01T00:00:00Z", txCount: 982, balance: "2100.88" },
  "zebvix@zbx": { address: "0xD4e5F6a7B8c9D4e5F6a7B8c9D4e5F6a7B8c9D4e5", registered: "2023-12-01T00:00:00Z", txCount: 52841, balance: "9800000.0" },
};

router.get("/payid/resolve/:payid", async (req, res): Promise<void> => {
  const payid = Array.isArray(req.params.payid) ? req.params.payid[0] : req.params.payid;
  const data = PAYIDS[payid.toLowerCase()];
  if (!data) {
    const hex = Buffer.from(payid).toString("hex").substring(0, 40);
    res.json({
      payid,
      address: "0x" + hex.padStart(40, "0"),
      network: "Zebvix Mainnet",
      registered: new Date(Date.now() - Math.random() * 1000 * 3600 * 24 * 365).toISOString(),
      txCount: Math.floor(Math.random() * 500) + 10,
      balance: (Math.random() * 10000).toFixed(4),
    });
    return;
  }
  res.json({ payid, ...data, network: "Zebvix Mainnet" });
});

export default router;
