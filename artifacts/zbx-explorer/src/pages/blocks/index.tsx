import React from "react";
import { Link } from "wouter";
import { useGetBlocks } from "@workspace/api-client-react";
import { formatNumber, timeAgo, formatAddress } from "@/lib/utils";
import { Skeleton } from "@/components/ui/skeleton";
import { ChevronLeft, ChevronRight, Blocks as BlocksIcon } from "lucide-react";

export default function Blocks() {
  const [page, setPage] = React.useState(0);
  const limit = 25;
  const { data: blocks, isLoading } = useGetBlocks({ limit, offset: page * limit });

  return (
    <div className="space-y-6">
      <div className="flex items-end justify-between">
        <div>
          <h2
            className="text-3xl font-black tracking-tight"
            style={{
              background: "linear-gradient(135deg, #00D4FF 0%, #0080FF 100%)",
              WebkitBackgroundClip: "text",
              WebkitTextFillColor: "transparent",
            }}
          >
            Blocks
          </h2>
          <p className="text-sm mt-1" style={{ color: "rgba(100,116,139,0.9)" }}>
            All blocks produced on the Zebvix network
          </p>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={() => setPage((p) => Math.max(0, p - 1))}
            disabled={page === 0}
            className="flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs font-semibold transition-all disabled:opacity-30"
            style={{ background: "rgba(0,212,255,0.08)", border: "1px solid rgba(0,212,255,0.15)", color: "#00D4FF" }}
          >
            <ChevronLeft className="w-3.5 h-3.5" /> Prev
          </button>
          <span className="text-xs font-mono px-3 py-1.5 rounded-lg" style={{ background: "rgba(0,212,255,0.04)", color: "#E2E8F0", border: "1px solid rgba(0,212,255,0.08)" }}>
            Page {page + 1}
          </span>
          <button
            onClick={() => setPage((p) => p + 1)}
            disabled={!blocks || blocks.length < limit}
            className="flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs font-semibold transition-all disabled:opacity-30"
            style={{ background: "rgba(0,212,255,0.08)", border: "1px solid rgba(0,212,255,0.15)", color: "#00D4FF" }}
          >
            Next <ChevronRight className="w-3.5 h-3.5" />
          </button>
        </div>
      </div>

      <div
        className="rounded-xl overflow-hidden"
        style={{ background: "rgba(10,22,40,0.7)", border: "1px solid rgba(0,212,255,0.1)", boxShadow: "0 4px 32px rgba(0,0,0,0.5)" }}
      >
        {isLoading ? (
          <div className="p-4 space-y-2">
            {Array.from({ length: 12 }).map((_, i) => <Skeleton key={i} className="h-11 w-full" />)}
          </div>
        ) : (
          <table className="w-full text-sm">
            <thead>
              <tr style={{ borderBottom: "1px solid rgba(0,212,255,0.08)" }}>
                {[["Block","left"],["Age","left"],["Txs","left"],["Hash","left"],["Proposer","left"],["Gas Used","right"],["Gas %","right"]].map(([h, align]) => (
                  <th key={h} className={`px-5 py-3.5 text-[10px] font-bold uppercase tracking-[0.1em] text-${align}`} style={{ color: "rgba(100,116,139,0.7)" }}>
                    {h}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {blocks?.map((block) => {
                const gasUsed = parseFloat(String(block.gasUsed));
                const gasLimit = parseFloat(String(block.gasLimit));
                const gasPct = gasLimit > 0 ? (gasUsed / gasLimit) * 100 : 0;
                return (
                  <tr key={block.number} className="premium-table-row">
                    <td className="px-5 py-3">
                      <Link href={`/blocks/${block.number}`} className="font-mono font-bold text-[13px]" style={{ color: "#00D4FF" }}>
                        {block.number}
                      </Link>
                    </td>
                    <td className="px-5 py-3 font-mono text-xs" style={{ color: "rgba(100,116,139,0.8)" }}>{timeAgo(block.timestamp)}</td>
                    <td className="px-5 py-3 font-mono text-sm font-semibold" style={{ color: "#E2E8F0" }}>{block.txCount}</td>
                    <td className="px-5 py-3 font-mono text-xs" style={{ color: "rgba(100,116,139,0.6)" }}>{formatAddress(block.hash, 8)}</td>
                    <td className="px-5 py-3 font-mono text-xs">
                      <Link href={`/validators/${block.proposer}`} className="hover:underline" style={{ color: "#4ADE80" }}>
                        {formatAddress(block.proposer, 6)}
                      </Link>
                    </td>
                    <td className="px-5 py-3 font-mono text-xs text-right" style={{ color: "rgba(148,163,184,0.8)" }}>{formatNumber(block.gasUsed, 0)}</td>
                    <td className="px-5 py-3 text-right">
                      <span className="font-mono text-xs font-bold px-2 py-0.5 rounded" style={{
                        background: gasPct > 80 ? "rgba(251,113,133,0.12)" : gasPct > 50 ? "rgba(252,211,77,0.12)" : "rgba(74,222,128,0.12)",
                        color: gasPct > 80 ? "#FB7185" : gasPct > 50 ? "#FCD34D" : "#4ADE80",
                      }}>
                        {gasPct.toFixed(1)}%
                      </span>
                    </td>
                  </tr>
                );
              })}
              {blocks?.length === 0 && (
                <tr><td colSpan={7} className="text-center py-16" style={{ color: "rgba(100,116,139,0.5)" }}>No blocks found.</td></tr>
              )}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
