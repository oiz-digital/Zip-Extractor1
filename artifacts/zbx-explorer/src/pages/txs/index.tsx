import React from "react";
import { Link } from "wouter";
import { useGetTransactions } from "@workspace/api-client-react";
import { formatNumber, timeAgo, formatAddress } from "@/lib/utils";
import { Skeleton } from "@/components/ui/skeleton";
import { ChevronLeft, ChevronRight, ArrowLeftRight } from "lucide-react";

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
  return (
    <span className={`inline-flex px-2 py-0.5 rounded text-[10px] font-bold uppercase border font-mono ${map[type] ?? "badge-transfer"}`}>
      {type}
    </span>
  );
}

function StatusBadge({ status }: { status: string }) {
  const map: Record<string, { bg: string; color: string }> = {
    success: { bg: "rgba(74,222,128,0.12)", color: "#4ADE80" },
    failed: { bg: "rgba(251,113,133,0.12)", color: "#FB7185" },
    pending: { bg: "rgba(252,211,77,0.12)", color: "#FCD34D" },
  };
  const s = map[status] ?? map.pending;
  return (
    <span className="font-mono text-[10px] font-bold uppercase px-2 py-0.5 rounded border" style={{ background: s.bg, color: s.color, borderColor: s.color + "40" }}>
      {status}
    </span>
  );
}

export default function Txs() {
  const [page, setPage] = React.useState(0);
  const limit = 25;
  const { data: txs, isLoading } = useGetTransactions({ limit, offset: page * limit });

  return (
    <div className="space-y-6">
      <div className="flex items-end justify-between">
        <div>
          <h2 className="text-3xl font-black tracking-tight" style={{ background: "linear-gradient(135deg, #00D4FF 0%, #0080FF 100%)", WebkitBackgroundClip: "text", WebkitTextFillColor: "transparent" }}>
            Transactions
          </h2>
          <p className="text-sm mt-1" style={{ color: "rgba(100,116,139,0.9)" }}>All transactions on the Zebvix network</p>
        </div>
        <div className="flex items-center gap-2">
          <button onClick={() => setPage((p) => Math.max(0, p - 1))} disabled={page === 0}
            className="flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs font-semibold disabled:opacity-30"
            style={{ background: "rgba(0,212,255,0.08)", border: "1px solid rgba(0,212,255,0.15)", color: "#00D4FF" }}>
            <ChevronLeft className="w-3.5 h-3.5" /> Prev
          </button>
          <span className="text-xs font-mono px-3 py-1.5 rounded-lg" style={{ background: "rgba(0,212,255,0.04)", color: "#E2E8F0", border: "1px solid rgba(0,212,255,0.08)" }}>
            Page {page + 1}
          </span>
          <button onClick={() => setPage((p) => p + 1)} disabled={!txs || txs.length < limit}
            className="flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs font-semibold disabled:opacity-30"
            style={{ background: "rgba(0,212,255,0.08)", border: "1px solid rgba(0,212,255,0.15)", color: "#00D4FF" }}>
            Next <ChevronRight className="w-3.5 h-3.5" />
          </button>
        </div>
      </div>

      <div className="rounded-xl overflow-hidden" style={{ background: "rgba(10,22,40,0.7)", border: "1px solid rgba(0,212,255,0.1)", boxShadow: "0 4px 32px rgba(0,0,0,0.5)" }}>
        {isLoading ? (
          <div className="p-4 space-y-2">{Array.from({ length: 12 }).map((_, i) => <Skeleton key={i} className="h-11 w-full" />)}</div>
        ) : (
          <table className="w-full text-sm">
            <thead>
              <tr style={{ borderBottom: "1px solid rgba(0,212,255,0.08)" }}>
                {[["Hash","left"],["Block","left"],["Age","left"],["From → To","left"],["Type","left"],["Status","left"],["Value","right"]].map(([h, align]) => (
                  <th key={h} className={`px-5 py-3.5 text-[10px] font-bold uppercase tracking-[0.1em] text-${align}`} style={{ color: "rgba(100,116,139,0.7)" }}>{h}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {txs?.map((tx) => (
                <tr key={tx.hash} className="premium-table-row">
                  <td className="px-5 py-3">
                    <Link href={`/txs/${tx.hash}`} className="font-mono text-[12px] font-bold" style={{ color: "#00D4FF" }}>
                      {formatAddress(tx.hash, 7)}
                    </Link>
                  </td>
                  <td className="px-5 py-3 font-mono text-xs">
                    <Link href={`/blocks/${tx.blockNumber}`} className="hover:underline" style={{ color: "rgba(148,163,184,0.8)" }}>{tx.blockNumber}</Link>
                  </td>
                  <td className="px-5 py-3 font-mono text-xs" style={{ color: "rgba(100,116,139,0.7)" }}>{timeAgo(tx.timestamp)}</td>
                  <td className="px-5 py-3 font-mono text-xs" style={{ color: "rgba(100,116,139,0.8)" }}>
                    <div className="flex flex-col gap-0.5">
                      <span style={{ color: "rgba(148,163,184,0.9)" }}>{formatAddress(tx.from, 6)}</span>
                      {tx.to && <span style={{ color: "rgba(100,116,139,0.6)" }}>→ {formatAddress(tx.to, 6)}</span>}
                    </div>
                  </td>
                  <td className="px-5 py-3"><TxTypeBadge type={tx.type} /></td>
                  <td className="px-5 py-3"><StatusBadge status={tx.status} /></td>
                  <td className="px-5 py-3 text-right font-mono text-xs font-semibold" style={{ color: "#E2E8F0" }}>
                    {formatNumber(tx.value, 2)} <span style={{ color: "rgba(100,116,139,0.5)" }}>ZBX</span>
                  </td>
                </tr>
              ))}
              {txs?.length === 0 && (
                <tr><td colSpan={7} className="text-center py-16" style={{ color: "rgba(100,116,139,0.5)" }}>No transactions found.</td></tr>
              )}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
