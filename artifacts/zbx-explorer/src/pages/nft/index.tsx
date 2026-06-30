import React from "react";
import { useGetNftCollections, useGetGamingProjects } from "@workspace/api-client-react";
import { formatNumber, formatCurrency, formatAddress } from "@/lib/utils";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Gamepad2, ImageIcon, CheckCircle2, Users, TrendingUp, Coins } from "lucide-react";

export default function NFT() {
  const { data: collections, isLoading: nftsLoading } = useGetNftCollections();
  const { data: games, isLoading: gamesLoading } = useGetGamingProjects();

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-3xl font-black tracking-tight" style={{ background: "linear-gradient(135deg, #A78BFA 0%, #60A5FA 100%)", WebkitBackgroundClip: "text", WebkitTextFillColor: "transparent" }}>
          NFT & Gaming
        </h2>
        <p className="text-sm mt-1" style={{ color: "rgba(100,116,139,0.9)" }}>Digital assets and interactive entertainment on Zebvix</p>
      </div>

      <Tabs defaultValue="nft" className="w-full">
        <TabsList className="grid w-full md:w-auto grid-cols-2" style={{ background: "rgba(0,212,255,0.05)", border: "1px solid rgba(0,212,255,0.1)" }}>
          <TabsTrigger value="nft" className="flex items-center gap-2"><ImageIcon className="w-4 h-4" /> NFT Collections</TabsTrigger>
          <TabsTrigger value="gaming" className="flex items-center gap-2"><Gamepad2 className="w-4 h-4" /> Gaming Projects</TabsTrigger>
        </TabsList>

        <TabsContent value="nft" className="mt-4">
          <div className="rounded-xl overflow-hidden" style={{ background: "rgba(10,22,40,0.7)", border: "1px solid rgba(139,92,246,0.12)", boxShadow: "0 4px 32px rgba(0,0,0,0.5)" }}>
            {nftsLoading ? (
              <div className="p-4 space-y-2">{Array.from({ length: 6 }).map((_, i) => <Skeleton key={i} className="h-14 w-full" />)}</div>
            ) : (
              <table className="w-full text-sm">
                <thead>
                  <tr style={{ borderBottom: "1px solid rgba(139,92,246,0.1)" }}>
                    {[["Collection","left"],["Floor Price","right"],["24h Volume","right"],["Supply","right"],["Owners","right"]].map(([h, a]) => (
                      <th key={h} className={`px-5 py-3.5 text-[10px] font-bold uppercase tracking-[0.1em] text-${a}`} style={{ color: "rgba(100,116,139,0.7)" }}>{h}</th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {collections?.map((col) => (
                    <tr key={col.id} className="premium-table-row">
                      <td className="px-5 py-4">
                        <div className="flex items-center gap-3">
                          <div className="w-10 h-10 rounded-xl flex items-center justify-center font-black text-sm flex-shrink-0"
                            style={{ background: "linear-gradient(135deg, rgba(139,92,246,0.2), rgba(96,165,250,0.2))", border: "1px solid rgba(139,92,246,0.2)", color: "#A78BFA" }}>
                            {col.symbol}
                          </div>
                          <div>
                            <div className="flex items-center gap-1.5 font-bold text-sm" style={{ color: "#E2E8F0" }}>
                              {col.name}
                              {col.verified && <CheckCircle2 className="w-3.5 h-3.5" style={{ color: "#00D4FF" }} />}
                            </div>
                            {col.contractAddress && (
                              <div className="font-mono text-[11px]" style={{ color: "rgba(100,116,139,0.6)" }}>{formatAddress(col.contractAddress, 6)}</div>
                            )}
                          </div>
                        </div>
                      </td>
                      <td className="px-5 py-4 text-right font-mono font-bold" style={{ color: "#A78BFA" }}>{formatNumber(col.floorPrice, 2)} ZBX</td>
                      <td className="px-5 py-4 text-right font-mono" style={{ color: "rgba(148,163,184,0.8)" }}>{formatNumber(col.volume24h, 2)} ZBX</td>
                      <td className="px-5 py-4 text-right font-mono" style={{ color: "rgba(148,163,184,0.8)" }}>{formatNumber(col.totalSupply, 0)}</td>
                      <td className="px-5 py-4 text-right font-mono" style={{ color: "rgba(148,163,184,0.8)" }}>{formatNumber(col.owners, 0)}</td>
                    </tr>
                  ))}
                  {(!collections || collections.length === 0) && (
                    <tr><td colSpan={5} className="text-center py-16" style={{ color: "rgba(100,116,139,0.5)" }}>No NFT collections found.</td></tr>
                  )}
                </tbody>
              </table>
            )}
          </div>
        </TabsContent>

        <TabsContent value="gaming" className="mt-4">
          <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
            {gamesLoading
              ? Array.from({ length: 4 }).map((_, i) => <Skeleton key={i} className="h-52 w-full rounded-xl" />)
              : games?.map((game) => (
                <div
                  key={game.id}
                  className="rounded-xl overflow-hidden transition-all duration-200 card-glow"
                  style={{ background: "linear-gradient(135deg, rgba(10,22,40,0.9) 0%, rgba(6,13,26,0.95) 100%)" }}
                >
                  <div className="h-1.5" style={{ background: "linear-gradient(90deg, #A78BFA, #60A5FA)" }} />
                  <div className="p-5">
                    <div className="flex justify-between items-start mb-3">
                      <div>
                        <h3 className="font-black text-lg" style={{ color: "#E2E8F0" }}>{game.name}</h3>
                        <span className="text-[10px] font-bold uppercase tracking-wider px-2 py-0.5 rounded mt-1 inline-block" style={{ background: "rgba(139,92,246,0.12)", color: "#A78BFA" }}>
                          {game.category}
                        </span>
                      </div>
                      {game.onChain && (
                        <span className="text-[11px] font-bold px-2.5 py-1 rounded-lg" style={{ background: "rgba(74,222,128,0.12)", color: "#4ADE80", border: "1px solid rgba(74,222,128,0.2)" }}>
                          Fully On-Chain
                        </span>
                      )}
                    </div>
                    <p className="text-sm mb-5" style={{ color: "rgba(100,116,139,0.8)" }}>{game.description}</p>
                    <div className="grid grid-cols-3 gap-3 pt-4" style={{ borderTop: "1px solid rgba(0,212,255,0.08)" }}>
                      <div className="text-center">
                        <Users className="w-4 h-4 mx-auto mb-1" style={{ color: "#A78BFA" }} />
                        <div className="font-mono font-black text-lg" style={{ color: "#E2E8F0" }}>{formatNumber(game.players, 0)}</div>
                        <div className="text-[10px] uppercase tracking-wider" style={{ color: "rgba(100,116,139,0.6)" }}>Players (24h)</div>
                      </div>
                      <div className="text-center">
                        <TrendingUp className="w-4 h-4 mx-auto mb-1" style={{ color: "#00D4FF" }} />
                        <div className="font-mono font-black text-lg" style={{ color: "#E2E8F0" }}>{formatNumber(game.txLast24h, 0)}</div>
                        <div className="text-[10px] uppercase tracking-wider" style={{ color: "rgba(100,116,139,0.6)" }}>Txs (24h)</div>
                      </div>
                      <div className="text-center">
                        <Coins className="w-4 h-4 mx-auto mb-1" style={{ color: "#4ADE80" }} />
                        <div className="font-mono font-black text-lg" style={{ color: "#4ADE80" }}>{formatCurrency(game.totalRevenue, 0)}</div>
                        <div className="text-[10px] uppercase tracking-wider" style={{ color: "rgba(100,116,139,0.6)" }}>Revenue</div>
                      </div>
                    </div>
                  </div>
                </div>
              ))}
            {!gamesLoading && (!games || games.length === 0) && (
              <div className="col-span-full text-center py-20 rounded-xl" style={{ border: "1px dashed rgba(0,212,255,0.15)", color: "rgba(100,116,139,0.5)" }}>No gaming projects found.</div>
            )}
          </div>
        </TabsContent>
      </Tabs>
    </div>
  );
}
