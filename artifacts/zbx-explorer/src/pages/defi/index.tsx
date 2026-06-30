import React from "react";
import { useGetDefiStats, useGetAmmPools, useGetLendingMarkets, useGetPerpMarkets } from "@workspace/api-client-react";
import { formatNumber, formatCurrency } from "@/lib/utils";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { DollarSign, Activity, PieChart, TrendingUp } from "lucide-react";

function StatCard({ label, value, icon: Icon, iconStyle, loading }: any) {
  return (
    <div className="rounded-xl p-5 flex items-center gap-4 card-glow" style={{ background: "linear-gradient(135deg, rgba(10,22,40,0.9) 0%, rgba(6,13,26,0.95) 100%)" }}>
      <div className="p-3 rounded-xl flex-shrink-0" style={iconStyle}><Icon className="w-5 h-5" /></div>
      <div>
        <p className="text-[10px] font-bold uppercase tracking-[0.12em]" style={{ color: "rgba(100,116,139,0.9)" }}>{label}</p>
        {loading ? <Skeleton className="h-7 w-28 mt-1" /> : (
          <p className="text-2xl font-black font-mono tracking-tight mt-0.5" style={{ color: "#E2E8F0" }}>{value}</p>
        )}
      </div>
    </div>
  );
}

function PremiumTable({ headers, children }: { headers: [string, string][]; children: React.ReactNode }) {
  return (
    <div className="rounded-xl overflow-hidden" style={{ background: "rgba(10,22,40,0.6)", border: "1px solid rgba(0,212,255,0.08)" }}>
      <table className="w-full text-sm">
        <thead>
          <tr style={{ borderBottom: "1px solid rgba(0,212,255,0.08)" }}>
            {headers.map(([h, align]) => (
              <th key={h} className={`px-5 py-3.5 text-[10px] font-bold uppercase tracking-[0.1em] text-${align}`} style={{ color: "rgba(100,116,139,0.7)" }}>{h}</th>
            ))}
          </tr>
        </thead>
        <tbody>{children}</tbody>
      </table>
    </div>
  );
}

export default function Defi() {
  const { data: stats, isLoading: statsLoading } = useGetDefiStats();
  const { data: amms, isLoading: ammsLoading } = useGetAmmPools();
  const { data: lending, isLoading: lendingLoading } = useGetLendingMarkets();
  const { data: perps, isLoading: perpsLoading } = useGetPerpMarkets();

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-3xl font-black tracking-tight" style={{ background: "linear-gradient(135deg, #4ADE80 0%, #00D4FF 100%)", WebkitBackgroundClip: "text", WebkitTextFillColor: "transparent" }}>
          DeFi Hub
        </h2>
        <p className="text-sm mt-1" style={{ color: "rgba(100,116,139,0.9)" }}>Native decentralized finance ecosystem on Zebvix</p>
      </div>

      <div className="grid grid-cols-2 lg:grid-cols-4 gap-4">
        <StatCard label="Total TVL" value={statsLoading ? "—" : formatCurrency(stats?.totalTvl || 0)}
          icon={DollarSign} iconStyle={{ background: "rgba(74,222,128,0.1)", color: "#4ADE80", boxShadow: "0 0 12px rgba(74,222,128,0.15)" }} loading={statsLoading} />
        <StatCard label="24h Volume" value={statsLoading ? "—" : formatCurrency(stats?.totalVolume24h || 0)}
          icon={Activity} iconStyle={{ background: "rgba(0,212,255,0.12)", color: "#00D4FF", boxShadow: "0 0 12px rgba(0,212,255,0.2)" }} loading={statsLoading} />
        <StatCard label="Protocols" value={statsLoading ? "—" : String(stats?.totalProtocols || 0)}
          icon={PieChart} iconStyle={{ background: "rgba(139,92,246,0.12)", color: "#A78BFA", boxShadow: "0 0 12px rgba(139,92,246,0.2)" }} loading={statsLoading} />
        <StatCard label="Perp Vol (24h)" value={statsLoading ? "—" : formatCurrency(stats?.perpVolume24h || 0)}
          icon={TrendingUp} iconStyle={{ background: "rgba(255,140,0,0.12)", color: "#FB923C", boxShadow: "0 0 12px rgba(255,140,0,0.15)" }} loading={statsLoading} />
      </div>

      <Tabs defaultValue="amm" className="w-full">
        <TabsList className="grid w-full md:w-auto grid-cols-3" style={{ background: "rgba(0,212,255,0.05)", border: "1px solid rgba(0,212,255,0.1)" }}>
          <TabsTrigger value="amm">AMM Pools</TabsTrigger>
          <TabsTrigger value="lending">Lending</TabsTrigger>
          <TabsTrigger value="perp">Perpetuals</TabsTrigger>
        </TabsList>

        <TabsContent value="amm" className="mt-4">
          {ammsLoading ? <div className="space-y-2">{Array.from({ length: 5 }).map((_, i) => <Skeleton key={i} className="h-12 w-full" />)}</div> : (
            <PremiumTable headers={[["Pool Pair","left"],["TVL","right"],["24h Volume","right"],["Fee","right"],["APY","right"]]}>
              {amms?.map((pool) => (
                <tr key={pool.id} className="premium-table-row">
                  <td className="px-5 py-3.5">
                    <div className="flex items-center gap-2">
                      <div className="flex -space-x-1">
                        <div className="w-6 h-6 rounded-full flex items-center justify-center text-[9px] font-bold" style={{ background: "rgba(0,212,255,0.2)", color: "#00D4FF" }}>{pool.token0?.[0]}</div>
                        <div className="w-6 h-6 rounded-full flex items-center justify-center text-[9px] font-bold" style={{ background: "rgba(74,222,128,0.2)", color: "#4ADE80" }}>{pool.token1?.[0]}</div>
                      </div>
                      <span className="font-bold text-sm">{pool.token0} / {pool.token1}</span>
                    </div>
                  </td>
                  <td className="px-5 py-3.5 text-right font-mono font-semibold" style={{ color: "#E2E8F0" }}>{formatCurrency(pool.tvl)}</td>
                  <td className="px-5 py-3.5 text-right font-mono" style={{ color: "rgba(148,163,184,0.8)" }}>{formatCurrency(pool.volume24h)}</td>
                  <td className="px-5 py-3.5 text-right font-mono text-xs" style={{ color: "rgba(100,116,139,0.8)" }}>{pool.fee}%</td>
                  <td className="px-5 py-3.5 text-right font-mono font-bold" style={{ color: "#4ADE80" }}>{pool.apy}%</td>
                </tr>
              ))}
            </PremiumTable>
          )}
        </TabsContent>

        <TabsContent value="lending" className="mt-4">
          {lendingLoading ? <div className="space-y-2">{Array.from({ length: 5 }).map((_, i) => <Skeleton key={i} className="h-12 w-full" />)}</div> : (
            <PremiumTable headers={[["Asset","left"],["Total Supply","right"],["Total Borrow","right"],["Utilization","left"],["Supply APY","right"],["Borrow APY","right"]]}>
              {lending?.map((market) => {
                const utilPct = (market.utilization || 0) * 100;
                return (
                  <tr key={market.asset} className="premium-table-row">
                    <td className="px-5 py-3.5">
                      <div className="flex items-center gap-2">
                        <div className="w-7 h-7 rounded-lg flex items-center justify-center text-[10px] font-bold" style={{ background: "rgba(139,92,246,0.15)", color: "#A78BFA" }}>{market.asset?.[0]}</div>
                        <span className="font-bold">{market.asset}</span>
                      </div>
                    </td>
                    <td className="px-5 py-3.5 text-right font-mono" style={{ color: "#E2E8F0" }}>{formatCurrency(market.totalSupply)}</td>
                    <td className="px-5 py-3.5 text-right font-mono" style={{ color: "rgba(148,163,184,0.8)" }}>{formatCurrency(market.totalBorrow)}</td>
                    <td className="px-5 py-3.5 w-44">
                      <div className="flex items-center gap-2">
                        <div className="flex-1 h-1.5 rounded-full" style={{ background: "rgba(0,212,255,0.1)" }}>
                          <div className="h-full rounded-full" style={{ width: `${utilPct}%`, background: utilPct > 80 ? "#FB7185" : utilPct > 60 ? "#FCD34D" : "#4ADE80" }} />
                        </div>
                        <span className="font-mono text-xs w-10" style={{ color: "rgba(148,163,184,0.8)" }}>{utilPct.toFixed(0)}%</span>
                      </div>
                    </td>
                    <td className="px-5 py-3.5 text-right font-mono font-bold" style={{ color: "#4ADE80" }}>{market.supplyApy}%</td>
                    <td className="px-5 py-3.5 text-right font-mono font-bold" style={{ color: "#FB7185" }}>{market.borrowApy}%</td>
                  </tr>
                );
              })}
            </PremiumTable>
          )}
        </TabsContent>

        <TabsContent value="perp" className="mt-4">
          {perpsLoading ? <div className="space-y-2">{Array.from({ length: 5 }).map((_, i) => <Skeleton key={i} className="h-12 w-full" />)}</div> : (
            <PremiumTable headers={[["Market","left"],["Price","right"],["24h Change","right"],["24h Volume","right"],["Open Interest","right"],["Funding (1h)","right"]]}>
              {perps?.map((market) => (
                <tr key={market.symbol} className="premium-table-row">
                  <td className="px-5 py-3.5 font-bold">{market.symbol}</td>
                  <td className="px-5 py-3.5 text-right font-mono font-semibold" style={{ color: "#E2E8F0" }}>{formatCurrency(market.price, 4)}</td>
                  <td className="px-5 py-3.5 text-right font-mono font-bold" style={{ color: market.change24h >= 0 ? "#4ADE80" : "#FB7185" }}>
                    {market.change24h > 0 ? "+" : ""}{market.change24h}%
                  </td>
                  <td className="px-5 py-3.5 text-right font-mono" style={{ color: "rgba(148,163,184,0.8)" }}>{formatCurrency(market.volume24h)}</td>
                  <td className="px-5 py-3.5 text-right font-mono" style={{ color: "rgba(148,163,184,0.8)" }}>{formatCurrency(market.openInterest)}</td>
                  <td className="px-5 py-3.5 text-right font-mono text-xs font-bold" style={{ color: market.fundingRate >= 0 ? "#4ADE80" : "#FB7185" }}>
                    {market.fundingRate}%
                  </td>
                </tr>
              ))}
            </PremiumTable>
          )}
        </TabsContent>
      </Tabs>
    </div>
  );
}
