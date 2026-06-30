import React from "react";
import { useGetOraclePrices } from "@workspace/api-client-react";
import { formatCurrency, timeAgo } from "@/lib/utils";
import { Skeleton } from "@/components/ui/skeleton";
import { CircleDot, TrendingUp, TrendingDown, Activity } from "lucide-react";

function PriceCard({ feed }: { feed: any }) {
  const price = typeof feed.price === "string" ? parseFloat(feed.price) : feed.price;
  const change = feed.change24h ?? 0;
  const isUp = change >= 0;

  return (
    <div
      className="rounded-xl p-5 transition-all duration-200 card-glow relative overflow-hidden"
      style={{ background: "linear-gradient(135deg, rgba(10,22,40,0.9) 0%, rgba(6,13,26,0.95) 100%)" }}
    >
      {/* Background gradient */}
      <div className="absolute top-0 right-0 w-20 h-20 opacity-[0.06] rounded-bl-full" style={{ background: isUp ? "#4ADE80" : "#FB7185" }} />

      <div className="flex justify-between items-start mb-3">
        <div>
          <h3 className="font-black text-base tracking-tight" style={{ color: "#E2E8F0" }}>{feed.pair}</h3>
          <div className="flex items-center gap-1 mt-0.5">
            <Activity className="w-3 h-3" style={{ color: "rgba(100,116,139,0.6)" }} />
            <span className="text-[10px] font-mono" style={{ color: "rgba(100,116,139,0.6)" }}>{feed.sources} sources</span>
          </div>
        </div>
        <div className="flex items-center gap-1 px-2 py-1 rounded-lg" style={{ background: isUp ? "rgba(74,222,128,0.1)" : "rgba(251,113,133,0.1)", border: `1px solid ${isUp ? "rgba(74,222,128,0.2)" : "rgba(251,113,133,0.2)"}` }}>
          {isUp ? <TrendingUp className="w-3 h-3" style={{ color: "#4ADE80" }} /> : <TrendingDown className="w-3 h-3" style={{ color: "#FB7185" }} />}
          <span className="font-mono text-xs font-bold" style={{ color: isUp ? "#4ADE80" : "#FB7185" }}>
            {isUp ? "+" : ""}{change}%
          </span>
        </div>
      </div>

      <div className="mb-4">
        <div className="font-black font-mono text-2xl tracking-tight" style={{ color: "#E2E8F0" }}>
          {formatCurrency(price, price < 1 ? 4 : 2)}
        </div>
      </div>

      <div className="grid grid-cols-2 gap-3 mb-3">
        <div className="rounded-lg p-2.5" style={{ background: "rgba(0,212,255,0.04)", border: "1px solid rgba(0,212,255,0.06)" }}>
          <div className="text-[10px] font-bold uppercase tracking-wider mb-1" style={{ color: "rgba(100,116,139,0.6)" }}>24h High</div>
          <div className="font-mono text-sm font-semibold" style={{ color: "#4ADE80" }}>{formatCurrency(feed.high24h, 2)}</div>
        </div>
        <div className="rounded-lg p-2.5" style={{ background: "rgba(0,212,255,0.04)", border: "1px solid rgba(0,212,255,0.06)" }}>
          <div className="text-[10px] font-bold uppercase tracking-wider mb-1" style={{ color: "rgba(100,116,139,0.6)" }}>24h Low</div>
          <div className="font-mono text-sm font-semibold" style={{ color: "#FB7185" }}>{formatCurrency(feed.low24h, 2)}</div>
        </div>
      </div>

      <div className="flex justify-between items-center pt-2" style={{ borderTop: "1px solid rgba(0,212,255,0.06)" }}>
        <span className="text-[10px] font-mono" style={{ color: "rgba(100,116,139,0.5)" }}>Dev: {feed.deviation ? `${feed.deviation}%` : "0.01%"}</span>
        <span className="text-[10px] font-mono" style={{ color: "rgba(100,116,139,0.5)" }}>{timeAgo(feed.lastUpdated)}</span>
      </div>
    </div>
  );
}

export default function Oracle() {
  const { data: prices, isLoading } = useGetOraclePrices();

  return (
    <div className="space-y-6">
      <div className="flex items-end justify-between">
        <div>
          <h2 className="text-3xl font-black tracking-tight" style={{ background: "linear-gradient(135deg, #FCD34D 0%, #FB923C 100%)", WebkitBackgroundClip: "text", WebkitTextFillColor: "transparent" }}>
            Oracle Price Feed
          </h2>
          <p className="text-sm mt-1" style={{ color: "rgba(100,116,139,0.9)" }}>
            Sub-second decentralized price feeds natively integrated into L1 consensus
          </p>
        </div>
        {!isLoading && prices && (
          <div className="flex items-center gap-2 px-3 py-1.5 rounded-lg" style={{ background: "rgba(74,222,128,0.08)", border: "1px solid rgba(74,222,128,0.15)" }}>
            <div className="w-1.5 h-1.5 rounded-full neon-dot" style={{ background: "#4ADE80" }} />
            <span className="text-xs font-semibold font-mono" style={{ color: "#4ADE80" }}>{prices.length} Live Feeds</span>
          </div>
        )}
      </div>

      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4">
        {isLoading
          ? Array.from({ length: 8 }).map((_, i) => <Skeleton key={i} className="h-52 w-full rounded-xl" />)
          : prices?.map((feed) => <PriceCard key={feed.pair} feed={feed} />)
        }
      </div>

      {!isLoading && (!prices || prices.length === 0) && (
        <div className="text-center py-20 rounded-xl" style={{ border: "1px dashed rgba(0,212,255,0.15)", color: "rgba(100,116,139,0.5)" }}>
          No active price feeds available.
        </div>
      )}
    </div>
  );
}
