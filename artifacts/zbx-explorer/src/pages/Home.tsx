import React from "react";
import { Link } from "wouter";
import { useGetNetworkOverview, useGetNetworkStats, useGetBlocks, useGetTransactions } from "@workspace/api-client-react";
import { formatNumber, timeAgo, formatAddress } from "@/lib/utils";
import { Activity, Box, Zap, Users, ChevronRight, TrendingUp, Shield, Globe } from "lucide-react";
import { Skeleton } from "@/components/ui/skeleton";

function StatCard({
  label,
  value,
  sub,
  icon: Icon,
  iconClass,
  cardClass,
  loading,
}: {
  label: string;
  value: string;
  sub?: string;
  icon: React.ElementType;
  iconClass: string;
  cardClass: string;
  loading?: boolean;
}) {
  return (
    <div
      className={`relative overflow-hidden rounded-xl p-5 flex items-center gap-4 ${cardClass}`}
      style={{ background: "linear-gradient(135deg, rgba(10,22,40,0.9) 0%, rgba(6,13,26,0.95) 100%)" }}
    >
      <div className={`p-3 rounded-xl flex-shrink-0 ${iconClass}`}>
        <Icon className="w-5 h-5" />
      </div>
      <div className="min-w-0">
        <p className="text-[10px] font-bold uppercase tracking-[0.12em]" style={{ color: "rgba(100,116,139,0.9)" }}>
          {label}
        </p>
        {loading ? (
          <Skeleton className="h-7 w-28 mt-1" />
        ) : (
          <p className="text-2xl font-black font-mono tracking-tight mt-0.5" style={{ color: "#E2E8F0" }}>
            {value}
          </p>
        )}
        {sub && !loading && (
          <p className="text-[11px] font-mono mt-0.5" style={{ color: "rgba(100,116,139,0.7)" }}>
            {sub}
          </p>
        )}
      </div>
      <div className="absolute top-0 right-0 w-24 h-24 opacity-[0.04] rounded-bl-full bg-white" />
    </div>
  );
}

function TxTypeBadge({ type }: { type: string }) {
  const map: Record<string, string> = {
    TRANSFER: "badge-transfer",
    AI_INFERENCE: "badge-ai",
    XCL_TRANSFER: "badge-xcl",
    CONTRACT_CALL: "badge-contract",
    CONTRACT_CREATION: "badge-contract",
    STAKE: "badge-defi",
    UNSTAKE: "badge-defi",
    SWAP: "badge-defi",
    GOVERNANCE: "badge-gov",
  };
  const cls = map[type] ?? "badge-transfer";
  return (
    <span className={`inline-flex items-center px-2 py-0.5 rounded text-[10px] font-bold uppercase border font-mono ${cls}`}>
      {type}
    </span>
  );
}

export default function Home() {
  const { data: overview, isLoading: overviewLoading } = useGetNetworkOverview();
  const { data: stats, isLoading: statsLoading } = useGetNetworkStats();
  const { data: blocks, isLoading: blocksLoading } = useGetBlocks({ limit: 8 });
  const { data: txs, isLoading: txsLoading } = useGetTransactions({ limit: 8 });

  return (
    <div className="space-y-8">
      {/* Page header */}
      <div>
        <h2
          className="text-3xl font-black tracking-tight"
          style={{
            background: "linear-gradient(135deg, #00D4FF 0%, #0080FF 55%, #8B5CF6 100%)",
            WebkitBackgroundClip: "text",
            WebkitTextFillColor: "transparent",
          }}
        >
          Network Dashboard
        </h2>
        <p className="text-sm mt-1" style={{ color: "rgba(100,116,139,0.9)" }}>
          Live Zebvix network statistics and activity — Chain ID 8989
        </p>
      </div>

      {/* Stats grid */}
      <div className="grid grid-cols-2 lg:grid-cols-4 gap-4">
        <StatCard
          label="Latest Block"
          value={statsLoading ? "—" : formatNumber(stats?.blockHeight || 0, 0)}
          sub="~5s block time"
          icon={Box}
          iconClass="icon-box-cyan"
          cardClass="stat-card-cyan"
          loading={statsLoading}
        />
        <StatCard
          label="Live TPS"
          value={overviewLoading ? "—" : formatNumber(overview?.tps || 0, 1)}
          sub="transactions/sec"
          icon={Zap}
          iconClass="icon-box-green"
          cardClass="stat-card-green"
          loading={overviewLoading}
        />
        <StatCard
          label="Finality"
          value={overviewLoading ? "—" : `${formatNumber(overview?.finalityTime || 0, 2)}s`}
          sub="BLS aggregate"
          icon={Shield}
          iconClass="icon-box-purple"
          cardClass="stat-card-purple"
          loading={overviewLoading}
        />
        <StatCard
          label="Validators"
          value={overviewLoading ? "—" : String(overview?.activeValidators || 0)}
          sub={overviewLoading ? "" : `${formatNumber((overview?.activeValidators || 0) / 100 * 100, 0)}% active`}
          icon={Users}
          iconClass="icon-box-orange"
          cardClass="stat-card-orange"
          loading={overviewLoading}
        />
      </div>

      {/* Secondary stats row */}
      <div className="grid grid-cols-2 lg:grid-cols-4 gap-3">
        {[
          { label: "Total Supply", value: "150,000,000 ZBX", color: "#00D4FF" },
          { label: "Market Cap", value: "$213M", color: "#4ADE80" },
          { label: "24h Volume", value: "$18.4M", color: "#A78BFA" },
          { label: "Bonded Ratio", value: "62.3%", color: "#FB923C" },
        ].map((s) => (
          <div
            key={s.label}
            className="rounded-lg px-4 py-3 flex justify-between items-center"
            style={{
              background: "rgba(0,212,255,0.03)",
              border: "1px solid rgba(0,212,255,0.08)",
            }}
          >
            <span className="text-[11px] uppercase tracking-wider font-semibold" style={{ color: "rgba(100,116,139,0.8)" }}>
              {s.label}
            </span>
            <span className="text-sm font-bold font-mono" style={{ color: s.color }}>
              {s.value}
            </span>
          </div>
        ))}
      </div>

      {/* Tables */}
      <div className="grid grid-cols-1 xl:grid-cols-2 gap-6">
        {/* Recent Blocks */}
        <div
          className="rounded-xl overflow-hidden"
          style={{
            background: "rgba(10,22,40,0.6)",
            border: "1px solid rgba(0,212,255,0.1)",
            boxShadow: "0 4px 24px rgba(0,0,0,0.4)",
          }}
        >
          <div
            className="px-5 py-4 flex items-center justify-between"
            style={{ borderBottom: "1px solid rgba(0,212,255,0.08)" }}
          >
            <div className="flex items-center gap-2">
              <Box className="w-4 h-4" style={{ color: "#00D4FF" }} />
              <span className="font-bold text-sm" style={{ color: "#E2E8F0" }}>Recent Blocks</span>
            </div>
            <Link href="/blocks" className="flex items-center gap-1 text-[11px] font-semibold" style={{ color: "#00D4FF" }}>
              View All <ChevronRight className="w-3 h-3" />
            </Link>
          </div>

          {blocksLoading ? (
            <div className="p-4 space-y-2">
              {Array.from({ length: 6 }).map((_, i) => <Skeleton key={i} className="h-11 w-full" />)}
            </div>
          ) : (
            <table className="w-full text-sm">
              <thead>
                <tr style={{ borderBottom: "1px solid rgba(0,212,255,0.06)" }}>
                  {["Block", "Age", "Txs", "Gas Used"].map((h, i) => (
                    <th
                      key={h}
                      className={`px-4 py-2.5 text-[10px] font-bold uppercase tracking-wider ${i === 3 ? "text-right" : "text-left"}`}
                      style={{ color: "rgba(100,116,139,0.7)" }}
                    >
                      {h}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {blocks?.slice(0, 8).map((block, idx) => (
                  <tr
                    key={block.number}
                    className="premium-table-row"
                    style={{ opacity: 1 - idx * 0.06 }}
                  >
                    <td className="px-4 py-2.5">
                      <Link href={`/blocks/${block.number}`} className="font-mono font-bold text-[13px]" style={{ color: "#00D4FF" }}>
                        {block.number}
                      </Link>
                    </td>
                    <td className="px-4 py-2.5 font-mono text-xs" style={{ color: "rgba(100,116,139,0.8)" }}>
                      {timeAgo(block.timestamp)}
                    </td>
                    <td className="px-4 py-2.5 font-mono text-sm" style={{ color: "#E2E8F0" }}>
                      {block.txCount}
                    </td>
                    <td className="px-4 py-2.5 font-mono text-xs text-right" style={{ color: "rgba(148,163,184,0.8)" }}>
                      {formatNumber(block.gasUsed, 0)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>

        {/* Recent Transactions */}
        <div
          className="rounded-xl overflow-hidden"
          style={{
            background: "rgba(10,22,40,0.6)",
            border: "1px solid rgba(0,212,255,0.1)",
            boxShadow: "0 4px 24px rgba(0,0,0,0.4)",
          }}
        >
          <div
            className="px-5 py-4 flex items-center justify-between"
            style={{ borderBottom: "1px solid rgba(0,212,255,0.08)" }}
          >
            <div className="flex items-center gap-2">
              <Activity className="w-4 h-4" style={{ color: "#4ADE80" }} />
              <span className="font-bold text-sm" style={{ color: "#E2E8F0" }}>Recent Transactions</span>
            </div>
            <Link href="/txs" className="flex items-center gap-1 text-[11px] font-semibold" style={{ color: "#00D4FF" }}>
              View All <ChevronRight className="w-3 h-3" />
            </Link>
          </div>

          {txsLoading ? (
            <div className="p-4 space-y-2">
              {Array.from({ length: 6 }).map((_, i) => <Skeleton key={i} className="h-11 w-full" />)}
            </div>
          ) : (
            <table className="w-full text-sm">
              <thead>
                <tr style={{ borderBottom: "1px solid rgba(0,212,255,0.06)" }}>
                  {["Hash", "Type", "Value"].map((h, i) => (
                    <th
                      key={h}
                      className={`px-4 py-2.5 text-[10px] font-bold uppercase tracking-wider ${i === 2 ? "text-right" : "text-left"}`}
                      style={{ color: "rgba(100,116,139,0.7)" }}
                    >
                      {h}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {txs?.slice(0, 8).map((tx, idx) => (
                  <tr
                    key={tx.hash}
                    className="premium-table-row"
                    style={{ opacity: 1 - idx * 0.06 }}
                  >
                    <td className="px-4 py-2.5">
                      <div className="flex flex-col gap-0.5">
                        <Link href={`/txs/${tx.hash}`} className="font-mono text-[12px] font-bold" style={{ color: "#00D4FF" }}>
                          {formatAddress(tx.hash, 6)}
                        </Link>
                        <span className="font-mono text-[10px]" style={{ color: "rgba(100,116,139,0.7)" }}>
                          {formatAddress(tx.from, 5)} → {tx.to ? formatAddress(tx.to, 5) : "new"}
                        </span>
                      </div>
                    </td>
                    <td className="px-4 py-2.5">
                      <TxTypeBadge type={tx.type} />
                    </td>
                    <td className="px-4 py-2.5 font-mono text-xs text-right font-semibold" style={{ color: "#E2E8F0" }}>
                      {formatNumber(tx.value, 2)} <span style={{ color: "rgba(100,116,139,0.6)" }}>ZBX</span>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      </div>
    </div>
  );
}
