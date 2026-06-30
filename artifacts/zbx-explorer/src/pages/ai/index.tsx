import React from "react";
import { Link } from "wouter";
import { useGetAiModels, useGetAiInferences, useGetAiStats } from "@workspace/api-client-react";
import { formatNumber, timeAgo, formatAddress } from "@/lib/utils";
import { Skeleton } from "@/components/ui/skeleton";
import { Brain, Zap, Activity, Cpu, Sparkles } from "lucide-react";

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

export default function AI() {
  const { data: stats, isLoading: statsLoading } = useGetAiStats();
  const { data: models, isLoading: modelsLoading } = useGetAiModels();
  const { data: inferences, isLoading: inferencesLoading } = useGetAiInferences({ limit: 12 });

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-3xl font-black tracking-tight" style={{ background: "linear-gradient(135deg, #A78BFA 0%, #00D4FF 100%)", WebkitBackgroundClip: "text", WebkitTextFillColor: "transparent" }}>
          On-Chain AI Inference
        </h2>
        <p className="text-sm mt-1" style={{ color: "rgba(100,116,139,0.9)" }}>
          Unique to Zebvix: trustless AI model inference embedded directly in L1 consensus
        </p>
      </div>

      <div className="grid grid-cols-2 lg:grid-cols-4 gap-4">
        <StatCard label="Total Inferences" value={statsLoading ? "—" : formatNumber(stats?.totalInferences || 0, 0)}
          icon={Activity} iconStyle={{ background: "rgba(139,92,246,0.12)", color: "#A78BFA", boxShadow: "0 0 12px rgba(139,92,246,0.2)" }} loading={statsLoading} />
        <StatCard label="Unique Callers" value={statsLoading ? "—" : formatNumber(stats?.uniqueCallers || 0, 0)}
          icon={Brain} iconStyle={{ background: "rgba(0,212,255,0.12)", color: "#00D4FF", boxShadow: "0 0 12px rgba(0,212,255,0.2)" }} loading={statsLoading} />
        <StatCard label="Top Model" value={statsLoading ? "—" : (stats?.topModel || "N/A")}
          icon={Cpu} iconStyle={{ background: "rgba(74,222,128,0.1)", color: "#4ADE80", boxShadow: "0 0 12px rgba(74,222,128,0.15)" }} loading={statsLoading} />
        <StatCard label="Avg Gas / Inference" value={statsLoading ? "—" : formatNumber(stats?.avgGasPerInference || 0, 0)}
          icon={Zap} iconStyle={{ background: "rgba(255,140,0,0.12)", color: "#FB923C", boxShadow: "0 0 12px rgba(255,140,0,0.15)" }} loading={statsLoading} />
      </div>

      <div className="grid grid-cols-1 xl:grid-cols-2 gap-6">
        {/* Registered Models */}
        <div className="rounded-xl overflow-hidden" style={{ background: "rgba(10,22,40,0.7)", border: "1px solid rgba(139,92,246,0.15)", boxShadow: "0 4px 32px rgba(0,0,0,0.5)" }}>
          <div className="px-5 py-4 flex items-center gap-2" style={{ borderBottom: "1px solid rgba(139,92,246,0.1)" }}>
            <Cpu className="w-4 h-4" style={{ color: "#A78BFA" }} />
            <span className="font-bold text-sm" style={{ color: "#E2E8F0" }}>Registered AI Models</span>
            <span className="ml-auto text-xs font-mono px-2 py-0.5 rounded" style={{ background: "rgba(139,92,246,0.12)", color: "#A78BFA" }}>
              {modelsLoading ? "..." : `${(models || []).length} models`}
            </span>
          </div>
          <div className="p-4 space-y-3 max-h-[500px] overflow-y-auto">
            {modelsLoading ? Array.from({ length: 4 }).map((_, i) => <Skeleton key={i} className="h-24 w-full" />) : (
              models?.map((model) => (
                <div key={model.id} className="rounded-lg p-4 transition-all" style={{ background: "rgba(139,92,246,0.05)", border: "1px solid rgba(139,92,246,0.12)" }}>
                  <div className="flex justify-between items-start mb-3">
                    <div>
                      <h4 className="font-bold text-sm" style={{ color: "#E2E8F0" }}>{model.name}</h4>
                      <p className="text-xs mt-0.5" style={{ color: "rgba(100,116,139,0.8)" }}>{model.description}</p>
                    </div>
                    <span className="text-[10px] font-bold uppercase px-2 py-0.5 rounded border font-mono" style={{ background: "rgba(139,92,246,0.15)", color: "#A78BFA", borderColor: "rgba(139,92,246,0.3)" }}>
                      {model.type}
                    </span>
                  </div>
                  <div className="grid grid-cols-3 gap-3 pt-3" style={{ borderTop: "1px solid rgba(139,92,246,0.08)" }}>
                    <div>
                      <span className="text-[10px] uppercase tracking-wider block mb-1" style={{ color: "rgba(100,116,139,0.7)" }}>Accuracy</span>
                      <span className="font-mono text-sm font-bold" style={{ color: "#4ADE80" }}>{((model.accuracy || 0) * 100).toFixed(1)}%</span>
                    </div>
                    <div>
                      <span className="text-[10px] uppercase tracking-wider block mb-1" style={{ color: "rgba(100,116,139,0.7)" }}>Gas Cost</span>
                      <span className="font-mono text-sm font-bold" style={{ color: "#E2E8F0" }}>{formatNumber(model.gasPerInference, 0)}</span>
                    </div>
                    <div>
                      <span className="text-[10px] uppercase tracking-wider block mb-1" style={{ color: "rgba(100,116,139,0.7)" }}>Calls</span>
                      <span className="font-mono text-sm font-bold" style={{ color: "#00D4FF" }}>{formatNumber(model.inferenceCount, 0)}</span>
                    </div>
                  </div>
                </div>
              ))
            )}
          </div>
        </div>

        {/* Recent Inferences */}
        <div className="rounded-xl overflow-hidden" style={{ background: "rgba(10,22,40,0.7)", border: "1px solid rgba(0,212,255,0.1)", boxShadow: "0 4px 32px rgba(0,0,0,0.5)" }}>
          <div className="px-5 py-4 flex items-center gap-2" style={{ borderBottom: "1px solid rgba(0,212,255,0.08)" }}>
            <Sparkles className="w-4 h-4" style={{ color: "#A78BFA" }} />
            <span className="font-bold text-sm" style={{ color: "#E2E8F0" }}>Recent Inferences</span>
          </div>
          <table className="w-full text-sm">
            <thead>
              <tr style={{ borderBottom: "1px solid rgba(0,212,255,0.08)" }}>
                {[["Tx / Caller","left"],["Model","left"],["Confidence","right"]].map(([h, a]) => (
                  <th key={h} className={`px-5 py-3 text-[10px] font-bold uppercase tracking-[0.1em] text-${a}`} style={{ color: "rgba(100,116,139,0.7)" }}>{h}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {inferencesLoading ? (
                <tr><td colSpan={3} className="p-4"><div className="space-y-2">{Array.from({ length: 6 }).map((_, i) => <Skeleton key={i} className="h-10 w-full" />)}</div></td></tr>
              ) : inferences?.map((inf) => {
                const conf = inf.confidence || 0;
                const confColor = conf > 0.9 ? "#4ADE80" : conf > 0.7 ? "#FCD34D" : "#FB7185";
                return (
                  <tr key={inf.txHash} className="premium-table-row">
                    <td className="px-5 py-3">
                      <div className="flex flex-col gap-0.5">
                        <Link href={`/txs/${inf.txHash}`} className="font-mono text-[12px] font-bold" style={{ color: "#00D4FF" }}>{formatAddress(inf.txHash, 6)}</Link>
                        <span className="font-mono text-[11px]" style={{ color: "rgba(100,116,139,0.6)" }}>{formatAddress(inf.caller, 6)}</span>
                        <span className="text-[10px]" style={{ color: "rgba(100,116,139,0.5)" }}>{timeAgo(inf.timestamp)}</span>
                      </div>
                    </td>
                    <td className="px-5 py-3">
                      <span className="text-[11px] font-bold px-2 py-0.5 rounded" style={{ background: "rgba(139,92,246,0.12)", color: "#A78BFA" }}>
                        {inf.modelName}
                      </span>
                    </td>
                    <td className="px-5 py-3 text-right font-mono font-bold" style={{ color: confColor }}>
                      {(conf * 100).toFixed(2)}%
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </div>
    </div>
  );
}
