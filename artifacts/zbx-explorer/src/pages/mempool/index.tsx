import React from "react";
import { Link } from "wouter";
import { useGetMempoolTxs, useGetMempoolStats } from "@workspace/api-client-react";
import { formatNumber, formatAddress, timeAgo } from "@/lib/utils";
import { Skeleton } from "@/components/ui/skeleton";
import { Clock, Activity, BarChart2, Layers, Zap } from "lucide-react";

function StatCard({ label, value, icon: Icon, iconStyle, loading }: any) {
  return (
    <div className="rounded-xl p-5 flex items-center gap-4 card-glow" style={{ background: "linear-gradient(135deg, rgba(10,22,40,0.9) 0%, rgba(6,13,26,0.95) 100%)" }}>
      <div className="p-3 rounded-xl flex-shrink-0" style={iconStyle}><Icon className="w-5 h-5" /></div>
      <div>
        <p className="text-[10px] font-bold uppercase tracking-[0.12em]" style={{ color: "rgba(100,116,139,0.9)" }}>{label}</p>
        {loading ? <Skeleton className="h-7 w-28 mt-1" /> : (
          <p className="text-xl font-black font-mono tracking-tight mt-0.5" style={{ color: "#E2E8F0" }}>{value}</p>
        )}
      </div>
    </div>
  );
}

function TxTypeBadge({ type }: { type: string }) {
  const map: Record<string, string> = {
    TRANSFER: "badge-transfer", AI_INFERENCE: "badge-ai", XCL_TRANSFER: "badge-xcl",
    CONTRACT_CALL: "badge-contract", CONTRACT_CREATION: "badge-contract", STAKE: "badge-defi",
  };
  return (
    <span className={`inline-flex px-2 py-0.5 rounded text-[10px] font-bold uppercase border font-mono ${map[type] ?? "badge-transfer"}`}>
      {type}
    </span>
  );
}

export default function Mempool() {
  const { data: stats, isLoading: statsLoading } = useGetMempoolStats();
  const { data: txs, isLoading: txsLoading } = useGetMempoolTxs({ limit: 50 });

  return (
    <div className="space-y-6">
      <div className="flex items-end justify-between">
        <div>
          <h2 className="text-3xl font-black tracking-tight" style={{ background: "linear-gradient(135deg, #FCD34D 0%, #FB923C 100%)", WebkitBackgroundClip: "text", WebkitTextFillColor: "transparent" }}>
            Mempool
          </h2>
          <p className="text-sm mt-1" style={{ color: "rgba(100,116,139,0.9)" }}>Live pending transactions waiting to be included in a block</p>
        </div>
        <div className="flex items-center gap-2 px-3 py-1.5 rounded-lg" style={{ background: "rgba(252,211,77,0.08)", border: "1px solid rgba(252,211,77,0.15)" }}>
          <div className="w-1.5 h-1.5 rounded-full" style={{ background: "#FCD34D", animation: "green-pulse 2s ease-in-out infinite" }} />
          <span className="text-xs font-semibold font-mono" style={{ color: "#FCD34D" }}>Live</span>
        </div>
      </div>

      <div className="grid grid-cols-2 lg:grid-cols-4 gap-4">
        <StatCard label="Pending / Queued" value={statsLoading ? "—" : `${formatNumber(stats?.pendingCount || 0, 0)} / ${formatNumber(stats?.queuedCount || 0, 0)}`}
          icon={Activity} iconStyle={{ background: "rgba(252,211,77,0.12)", color: "#FCD34D", boxShadow: "0 0 12px rgba(252,211,77,0.15)" }} loading={statsLoading} />
        <StatCard label="Avg Gas Price" value={statsLoading ? "—" : `${stats?.avgGasPrice || 0} gwei`}
          icon={BarChart2} iconStyle={{ background: "rgba(0,212,255,0.12)", color: "#00D4FF", boxShadow: "0 0 12px rgba(0,212,255,0.2)" }} loading={statsLoading} />
        <StatCard label="Min Gas Price" value={statsLoading ? "—" : `${stats?.minGasPrice || 0} gwei`}
          icon={Zap} iconStyle={{ background: "rgba(74,222,128,0.1)", color: "#4ADE80", boxShadow: "0 0 12px rgba(74,222,128,0.15)" }} loading={statsLoading} />
        <StatCard label="Oldest Tx Age" value={statsLoading ? "—" : `${stats?.oldestTxAge || 0}s`}
          icon={Clock} iconStyle={{ background: "rgba(251,113,133,0.12)", color: "#FB7185", boxShadow: "0 0 12px rgba(251,113,133,0.15)" }} loading={statsLoading} />
      </div>

      <div className="rounded-xl overflow-hidden" style={{ background: "rgba(10,22,40,0.7)", border: "1px solid rgba(252,211,77,0.08)", boxShadow: "0 4px 32px rgba(0,0,0,0.5)" }}>
        {txsLoading ? (
          <div className="p-4 space-y-2">{Array.from({ length: 12 }).map((_, i) => <Skeleton key={i} className="h-12 w-full" />)}</div>
        ) : (
          <table className="w-full text-sm">
            <thead>
              <tr style={{ borderBottom: "1px solid rgba(252,211,77,0.08)" }}>
                {[["Tx Hash","left"],["Time in Pool","left"],["From → To","left"],["Type","left"],["Gas Price","right"],["Value","right"]].map(([h, a]) => (
                  <th key={h} className={`px-5 py-3.5 text-[10px] font-bold uppercase tracking-[0.1em] text-${a}`} style={{ color: "rgba(100,116,139,0.7)" }}>{h}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {txs?.map((tx) => (
                <tr key={tx.hash} className="premium-table-row" style={{ opacity: 0.92 }}>
                  <td className="px-5 py-3 font-mono text-xs" style={{ color: "rgba(148,163,184,0.7)" }}>{formatAddress(tx.hash, 8)}</td>
                  <td className="px-5 py-3">
                    <div className="flex items-center gap-1.5 font-mono text-xs" style={{ color: "#FCD34D" }}>
                      <Clock className="w-3 h-3" />
                      {timeAgo(tx.addedAt)}
                    </div>
                  </td>
                  <td className="px-5 py-3 font-mono text-xs">
                    <div className="flex flex-col gap-0.5">
                      <span style={{ color: "rgba(148,163,184,0.9)" }}>{formatAddress(tx.from, 6)}</span>
                      {tx.to && <span style={{ color: "rgba(100,116,139,0.6)" }}>→ {formatAddress(tx.to, 6)}</span>}
                    </div>
                  </td>
                  <td className="px-5 py-3"><TxTypeBadge type={tx.type} /></td>
                  <td className="px-5 py-3 text-right font-mono text-xs font-semibold" style={{ color: "#00D4FF" }}>{tx.gasPrice} gwei</td>
                  <td className="px-5 py-3 text-right font-mono text-xs" style={{ color: "#E2E8F0" }}>
                    {formatNumber(tx.value, 4)} <span style={{ color: "rgba(100,116,139,0.5)" }}>ZBX</span>
                  </td>
                </tr>
              ))}
              {(!txs || txs.length === 0) && (
                <tr><td colSpan={6} className="text-center py-16" style={{ color: "rgba(100,116,139,0.5)" }}>Mempool is empty.</td></tr>
              )}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
