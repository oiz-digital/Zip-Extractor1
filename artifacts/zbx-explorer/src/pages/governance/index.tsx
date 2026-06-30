import React from "react";
import { Link } from "wouter";
import { useGetProposals } from "@workspace/api-client-react";
import { timeAgo } from "@/lib/utils";
import { Skeleton } from "@/components/ui/skeleton";
import { Vote, CheckCircle2, XCircle, Clock, AlertCircle } from "lucide-react";

function StatusBadge({ status }: { status: string }) {
  const map: Record<string, { bg: string; color: string; icon: React.ElementType }> = {
    active: { bg: "rgba(0,212,255,0.12)", color: "#00D4FF", icon: Activity },
    passed: { bg: "rgba(74,222,128,0.12)", color: "#4ADE80", icon: CheckCircle2 },
    rejected: { bg: "rgba(251,113,133,0.12)", color: "#FB7185", icon: XCircle },
    pending: { bg: "rgba(252,211,77,0.12)", color: "#FCD34D", icon: Clock },
    voting: { bg: "rgba(0,212,255,0.12)", color: "#00D4FF", icon: Vote },
  };
  const s = map[status.toLowerCase()] ?? map.pending;
  return (
    <span className="inline-flex items-center gap-1.5 font-mono text-[11px] font-bold uppercase px-2.5 py-1 rounded-lg border" style={{ background: s.bg, color: s.color, borderColor: s.color + "40" }}>
      {status}
    </span>
  );
}

function Activity(props: any) { return null; }

function VoteBar({ yes, no, abstain }: { yes: number; no: number; abstain: number }) {
  const total = yes + no + abstain;
  if (total === 0) return <div className="h-1.5 rounded-full" style={{ background: "rgba(0,212,255,0.1)" }} />;
  const yesPct = (yes / total) * 100;
  const noPct = (no / total) * 100;
  const abstainPct = (abstain / total) * 100;
  return (
    <div>
      <div className="flex h-2 rounded-full overflow-hidden" style={{ background: "rgba(0,212,255,0.06)" }}>
        <div style={{ width: `${yesPct}%`, background: "linear-gradient(90deg, #4ADE80, #22C55E)" }} />
        <div style={{ width: `${noPct}%`, background: "linear-gradient(90deg, #FB7185, #EF4444)" }} />
        <div style={{ width: `${abstainPct}%`, background: "rgba(252,211,77,0.6)" }} />
      </div>
      <div className="flex justify-between mt-1.5 text-[10px] font-mono">
        <span style={{ color: "#4ADE80" }}>Yes {yesPct.toFixed(1)}%</span>
        <span style={{ color: "#FB7185" }}>No {noPct.toFixed(1)}%</span>
        <span style={{ color: "#FCD34D" }}>Abstain {abstainPct.toFixed(1)}%</span>
      </div>
    </div>
  );
}

export default function Governance() {
  const { data: proposals, isLoading } = useGetProposals();

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-3xl font-black tracking-tight" style={{ background: "linear-gradient(135deg, #FB923C 0%, #FCD34D 100%)", WebkitBackgroundClip: "text", WebkitTextFillColor: "transparent" }}>
          Governance (ZEPs)
        </h2>
        <p className="text-sm mt-1" style={{ color: "rgba(100,116,139,0.9)" }}>Zebvix Evolution Proposals — on-chain voting and protocol upgrades</p>
      </div>

      <div className="space-y-3">
        {isLoading
          ? Array.from({ length: 5 }).map((_, i) => <Skeleton key={i} className="h-32 w-full rounded-xl" />)
          : proposals?.map((proposal) => {
              const yes = parseFloat(String(proposal.yesVotes)) || 0;
              const no = parseFloat(String(proposal.noVotes)) || 0;
              const abstain = parseFloat(String(proposal.abstainVotes)) || 0;
              const zepId = proposal.zepNumber ? `ZEP-${proposal.zepNumber}` : `#${proposal.id}`;
              return (
                <div
                  key={proposal.id}
                  className="rounded-xl p-5 transition-all duration-200"
                  style={{ background: "rgba(10,22,40,0.7)", border: "1px solid rgba(0,212,255,0.08)", boxShadow: "0 2px 16px rgba(0,0,0,0.4)" }}
                >
                  <div className="flex flex-col md:flex-row md:items-start gap-4">
                    <div className="flex-1 min-w-0">
                      <div className="flex items-center gap-3 mb-2">
                        <span className="font-mono text-sm font-black" style={{ color: "#FB923C" }}>{zepId}</span>
                        <StatusBadge status={proposal.status} />
                        <span className="ml-auto text-xs font-mono" style={{ color: "rgba(100,116,139,0.5)" }}>
                          Ends {new Date(proposal.endTime).toLocaleDateString()} · {timeAgo(proposal.endTime)}
                        </span>
                      </div>
                      <Link href={`/governance/${proposal.id}`} className="font-bold text-lg hover:underline block mb-1" style={{ color: "#E2E8F0" }}>
                        {proposal.title}
                      </Link>
                      <span className="text-[10px] font-bold uppercase tracking-wider px-2 py-0.5 rounded font-mono" style={{ background: "rgba(251,146,60,0.12)", color: "#FB923C" }}>
                        {proposal.type}
                      </span>
                    </div>
                    <div className="md:w-72 flex-shrink-0">
                      <VoteBar yes={yes} no={no} abstain={abstain} />
                    </div>
                  </div>
                </div>
              );
            })}
        {!isLoading && (!proposals || proposals.length === 0) && (
          <div className="text-center py-20 rounded-xl" style={{ border: "1px dashed rgba(0,212,255,0.15)", color: "rgba(100,116,139,0.5)" }}>
            No proposals found.
          </div>
        )}
      </div>
    </div>
  );
}
