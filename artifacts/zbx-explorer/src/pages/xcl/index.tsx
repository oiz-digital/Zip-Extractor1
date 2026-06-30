import React from "react";
import { Link } from "wouter";
import { useGetXclStats, useGetXclTransfers } from "@workspace/api-client-react";
import { formatNumber, formatCurrency, timeAgo, formatAddress } from "@/lib/utils";
import { Skeleton } from "@/components/ui/skeleton";
import { GitBranch, Activity, Globe, Clock, ArrowRight } from "lucide-react";

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

const chainColors: Record<string, string> = {
  Ethereum: "#627EEA", Bitcoin: "#F7931A", Cosmos: "#2E3148", Solana: "#9945FF",
  Avalanche: "#E84142", Polygon: "#8247E5", BSC: "#F3BA2F", ZBX: "#00D4FF",
};

function ChainBadge({ name }: { name: string }) {
  const color = chainColors[name] || "#00D4FF";
  return (
    <span className="font-mono text-[10px] font-bold px-2 py-0.5 rounded border" style={{ background: color + "15", color, borderColor: color + "40" }}>
      {name}
    </span>
  );
}

function StatusBadge({ status }: { status: string }) {
  const map: Record<string, { bg: string; color: string }> = {
    finalized: { bg: "rgba(74,222,128,0.12)", color: "#4ADE80" },
    pending: { bg: "rgba(252,211,77,0.12)", color: "#FCD34D" },
    failed: { bg: "rgba(251,113,133,0.12)", color: "#FB7185" },
  };
  const s = map[status] ?? map.pending;
  return (
    <span className="font-mono text-[10px] font-bold uppercase px-2 py-0.5 rounded border" style={{ background: s.bg, color: s.color, borderColor: s.color + "40" }}>
      {status}
    </span>
  );
}

export default function Xcl() {
  const { data: stats, isLoading: statsLoading } = useGetXclStats();
  const { data: transfers, isLoading: transfersLoading } = useGetXclTransfers({ limit: 20 });

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-3xl font-black tracking-tight" style={{ background: "linear-gradient(135deg, #67E8F9 0%, #00D4FF 100%)", WebkitBackgroundClip: "text", WebkitTextFillColor: "transparent" }}>
          Cross-Chain (XCL)
        </h2>
        <p className="text-sm mt-1" style={{ color: "rgba(100,116,139,0.9)" }}>Native trustless bridging and cross-chain message passing</p>
      </div>

      <div className="grid grid-cols-2 lg:grid-cols-4 gap-4">
        <StatCard label="Total Transfers" value={statsLoading ? "—" : formatNumber(stats?.totalTransfers || 0, 0)}
          icon={Activity} iconStyle={{ background: "rgba(103,232,249,0.12)", color: "#67E8F9", boxShadow: "0 0 12px rgba(103,232,249,0.2)" }} loading={statsLoading} />
        <StatCard label="24h Volume" value={statsLoading ? "—" : formatCurrency(stats?.volume24h || 0)}
          icon={Globe} iconStyle={{ background: "rgba(0,212,255,0.12)", color: "#00D4FF", boxShadow: "0 0 12px rgba(0,212,255,0.2)" }} loading={statsLoading} />
        <StatCard label="Connected Chains" value={statsLoading ? "—" : String(stats?.supportedChains || 0)}
          icon={GitBranch} iconStyle={{ background: "rgba(139,92,246,0.12)", color: "#A78BFA", boxShadow: "0 0 12px rgba(139,92,246,0.2)" }} loading={statsLoading} />
        <StatCard label="Avg Finality" value={statsLoading ? "—" : `${formatNumber(stats?.avgFinalizationTime || 0, 1)}s`}
          icon={Clock} iconStyle={{ background: "rgba(74,222,128,0.1)", color: "#4ADE80", boxShadow: "0 0 12px rgba(74,222,128,0.15)" }} loading={statsLoading} />
      </div>

      <div className="rounded-xl overflow-hidden" style={{ background: "rgba(10,22,40,0.7)", border: "1px solid rgba(0,212,255,0.1)", boxShadow: "0 4px 32px rgba(0,0,0,0.5)" }}>
        {transfersLoading ? (
          <div className="p-4 space-y-2">{Array.from({ length: 10 }).map((_, i) => <Skeleton key={i} className="h-12 w-full" />)}</div>
        ) : (
          <table className="w-full text-sm">
            <thead>
              <tr style={{ borderBottom: "1px solid rgba(0,212,255,0.08)" }}>
                {[["Tx Hash","left"],["Age","left"],["Path","left"],["Asset","left"],["Amount","right"],["Status","left"],["Proof","left"]].map(([h, a]) => (
                  <th key={h} className={`px-5 py-3.5 text-[10px] font-bold uppercase tracking-[0.1em] text-${a}`} style={{ color: "rgba(100,116,139,0.7)" }}>{h}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {transfers?.map((tx) => (
                <tr key={tx.id} className="premium-table-row">
                  <td className="px-5 py-3">
                    <Link href={`/txs/${tx.txHash}`} className="font-mono text-[12px] font-bold" style={{ color: "#00D4FF" }}>{formatAddress(tx.txHash, 6)}</Link>
                  </td>
                  <td className="px-5 py-3 font-mono text-xs" style={{ color: "rgba(100,116,139,0.7)" }}>{timeAgo(tx.timestamp)}</td>
                  <td className="px-5 py-3">
                    <div className="flex items-center gap-2">
                      <ChainBadge name={tx.sourceChain} />
                      <ArrowRight className="w-3 h-3" style={{ color: "rgba(100,116,139,0.5)" }} />
                      <ChainBadge name={tx.destChain} />
                    </div>
                  </td>
                  <td className="px-5 py-3 font-bold text-sm" style={{ color: "#E2E8F0" }}>{tx.asset}</td>
                  <td className="px-5 py-3 text-right font-mono text-sm font-semibold" style={{ color: "#E2E8F0" }}>{formatNumber(tx.amount, 4)}</td>
                  <td className="px-5 py-3"><StatusBadge status={tx.status} /></td>
                  <td className="px-5 py-3 font-mono text-xs uppercase" style={{ color: "rgba(100,116,139,0.6)" }}>{tx.proofType || "SPV"}</td>
                </tr>
              ))}
              {transfers?.length === 0 && (
                <tr><td colSpan={7} className="text-center py-16" style={{ color: "rgba(100,116,139,0.5)" }}>No recent transfers.</td></tr>
              )}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
