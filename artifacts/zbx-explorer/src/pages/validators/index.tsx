import React from "react";
import { Link } from "wouter";
import { useGetValidators, useGetValidatorStats } from "@workspace/api-client-react";
import { formatNumber, formatAddress } from "@/lib/utils";
import { Skeleton } from "@/components/ui/skeleton";
import { Users, ShieldCheck, Activity, TrendingUp } from "lucide-react";

function StatCard({ label, value, sub, icon: Icon, iconStyle, loading }: any) {
  return (
    <div className="rounded-xl p-5 flex items-center gap-4 card-glow" style={{ background: "linear-gradient(135deg, rgba(10,22,40,0.9) 0%, rgba(6,13,26,0.95) 100%)" }}>
      <div className="p-3 rounded-xl flex-shrink-0" style={iconStyle}><Icon className="w-5 h-5" /></div>
      <div>
        <p className="text-[10px] font-bold uppercase tracking-[0.12em]" style={{ color: "rgba(100,116,139,0.9)" }}>{label}</p>
        {loading ? <Skeleton className="h-7 w-28 mt-1" /> : (
          <p className="text-2xl font-black font-mono tracking-tight mt-0.5" style={{ color: "#E2E8F0" }}>{value}</p>
        )}
        {sub && !loading && <p className="text-[11px] font-mono mt-0.5" style={{ color: "rgba(100,116,139,0.6)" }}>{sub}</p>}
      </div>
    </div>
  );
}

export default function Validators() {
  const { data: validators, isLoading: validatorsLoading } = useGetValidators();
  const { data: stats, isLoading: statsLoading } = useGetValidatorStats();

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-3xl font-black tracking-tight" style={{ background: "linear-gradient(135deg, #4ADE80 0%, #00D4FF 100%)", WebkitBackgroundClip: "text", WebkitTextFillColor: "transparent" }}>
          Validators & Staking
        </h2>
        <p className="text-sm mt-1" style={{ color: "rgba(100,116,139,0.9)" }}>
          Network validators, staking distribution, and security metrics
        </p>
      </div>

      <div className="grid grid-cols-2 lg:grid-cols-4 gap-4">
        <StatCard label="Active / Total" value={statsLoading ? "—" : `${stats?.activeValidators} / ${stats?.totalValidators}`}
          icon={Users} iconStyle={{ background: "rgba(0,212,255,0.12)", color: "#00D4FF", boxShadow: "0 0 12px rgba(0,212,255,0.2)" }} loading={statsLoading} />
        <StatCard label="Total Staked" value={statsLoading ? "—" : formatNumber(stats?.totalStaked || 0, 0) + " ZBX"}
          sub="bonded supply" icon={ShieldCheck} iconStyle={{ background: "rgba(74,222,128,0.1)", color: "#4ADE80", boxShadow: "0 0 12px rgba(74,222,128,0.15)" }} loading={statsLoading} />
        <StatCard label="Staking APR" value={statsLoading ? "—" : `${formatNumber(stats?.annualizedReward || 0, 2)}%`}
          sub="annualized reward" icon={TrendingUp} iconStyle={{ background: "rgba(139,92,246,0.12)", color: "#A78BFA", boxShadow: "0 0 12px rgba(139,92,246,0.2)" }} loading={statsLoading} />
        <StatCard label="Bonded Ratio" value={statsLoading ? "—" : `${formatNumber(stats?.bondedRatio || 0, 2)}%`}
          icon={Activity} iconStyle={{ background: "rgba(255,140,0,0.12)", color: "#FB923C", boxShadow: "0 0 12px rgba(255,140,0,0.15)" }} loading={statsLoading} />
      </div>

      <div className="rounded-xl overflow-hidden" style={{ background: "rgba(10,22,40,0.7)", border: "1px solid rgba(0,212,255,0.1)", boxShadow: "0 4px 32px rgba(0,0,0,0.5)" }}>
        {validatorsLoading ? (
          <div className="p-4 space-y-2">{Array.from({ length: 10 }).map((_, i) => <Skeleton key={i} className="h-12 w-full" />)}</div>
        ) : (
          <table className="w-full text-sm">
            <thead>
              <tr style={{ borderBottom: "1px solid rgba(0,212,255,0.08)" }}>
                {[["Rank","left"],["Validator","left"],["Status","left"],["Voting Power","right"],["Commission","right"],["Uptime","right"]].map(([h, align]) => (
                  <th key={h} className={`px-5 py-3.5 text-[10px] font-bold uppercase tracking-[0.1em] text-${align}`} style={{ color: "rgba(100,116,139,0.7)" }}>{h}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {validators?.map((validator, index) => {
                const uptime = validator.uptime;
                const uptimeColor = uptime > 99 ? "#4ADE80" : uptime > 95 ? "#FCD34D" : "#FB7185";
                return (
                  <tr key={validator.address} className="premium-table-row">
                    <td className="px-5 py-3.5 font-mono text-sm font-bold" style={{ color: "rgba(100,116,139,0.6)" }}>
                      #{validator.rank || index + 1}
                    </td>
                    <td className="px-5 py-3.5">
                      <div className="flex flex-col gap-0.5">
                        <Link href={`/validators/${validator.address}`} className="font-bold text-sm hover:underline" style={{ color: "#00D4FF" }}>
                          {validator.moniker || "Unknown"}
                        </Link>
                        <span className="font-mono text-[11px]" style={{ color: "rgba(100,116,139,0.6)" }}>{formatAddress(validator.address, 10)}</span>
                      </div>
                    </td>
                    <td className="px-5 py-3.5">
                      <span className="font-mono text-[11px] font-bold uppercase px-2.5 py-1 rounded border"
                        style={validator.status === "active"
                          ? { background: "rgba(74,222,128,0.12)", color: "#4ADE80", borderColor: "rgba(74,222,128,0.3)" }
                          : { background: "rgba(100,116,139,0.1)", color: "#94A3B8", borderColor: "rgba(100,116,139,0.2)" }}>
                        {validator.status}
                      </span>
                    </td>
                    <td className="px-5 py-3.5 text-right font-mono text-sm font-semibold" style={{ color: "#E2E8F0" }}>
                      {formatNumber(validator.votingPower, 0)}
                    </td>
                    <td className="px-5 py-3.5 text-right font-mono text-sm" style={{ color: "rgba(148,163,184,0.8)" }}>
                      {formatNumber(validator.commission * 100, 1)}%
                    </td>
                    <td className="px-5 py-3.5 text-right font-mono text-sm font-bold" style={{ color: uptimeColor }}>
                      {formatNumber(uptime, 2)}%
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
