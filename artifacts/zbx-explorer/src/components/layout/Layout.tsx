import React, { useState, useEffect, useRef } from "react";
import { Link, useLocation } from "wouter";
import { useNetwork, NETWORKS, type NetworkId } from "@/context/NetworkContext";
import {
  LayoutDashboard,
  Blocks,
  ArrowLeftRight,
  Users,
  TrendingUp,
  Brain,
  GitBranch,
  CircleDot,
  Vote,
  Gamepad2,
  Clock,
  Search,
  Wifi,
  ChevronRight,
  ChevronDown,
  FlaskConical,
  Globe,
  CheckCircle2,
} from "lucide-react";

interface NavItem {
  name: string;
  path: string;
  icon: React.ElementType;
  group: string;
}

const navItems: NavItem[] = [
  { name: "Dashboard", path: "/", icon: LayoutDashboard, group: "Network" },
  { name: "Blocks", path: "/blocks", icon: Blocks, group: "Network" },
  { name: "Transactions", path: "/txs", icon: ArrowLeftRight, group: "Network" },
  { name: "Mempool", path: "/mempool", icon: Clock, group: "Network" },
  { name: "Validators", path: "/validators", icon: Users, group: "Staking" },
  { name: "DeFi Hub", path: "/defi", icon: TrendingUp, group: "Ecosystem" },
  { name: "On-Chain AI", path: "/ai", icon: Brain, group: "Ecosystem" },
  { name: "Cross-Chain", path: "/xcl", icon: GitBranch, group: "Ecosystem" },
  { name: "Oracle Prices", path: "/oracle", icon: CircleDot, group: "Ecosystem" },
  { name: "Governance", path: "/governance", icon: Vote, group: "Governance" },
  { name: "NFT & Gaming", path: "/nft", icon: Gamepad2, group: "Governance" },
];

const groupOrder = ["Network", "Staking", "Ecosystem", "Governance"];

const NAV_COLORS: Record<string, string> = {
  Network: "#00D4FF",
  Staking: "#4ADE80",
  Ecosystem: "#A78BFA",
  Governance: "#FB923C",
};

export default function Layout({ children }: { children: React.ReactNode }) {
  const [location] = useLocation();
  const [searchValue, setSearchValue] = useState("");
  const [networkOpen, setNetworkOpen] = useState(false);
  const [blockHeight, setBlockHeight] = useState(4872371);
  const dropdownRef = useRef<HTMLDivElement>(null);
  const { network, networkId, setNetwork } = useNetwork();

  const isTestnet = networkId === "testnet";
  const primaryColor = network.primary;

  useEffect(() => {
    const iv = setInterval(() => setBlockHeight((h) => h + 1), 5000);
    return () => clearInterval(iv);
  }, []);

  /* Close dropdown on outside click */
  useEffect(() => {
    function handle(e: MouseEvent) {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setNetworkOpen(false);
      }
    }
    document.addEventListener("mousedown", handle);
    return () => document.removeEventListener("mousedown", handle);
  }, []);

  const grouped = groupOrder.map((g) => ({
    label: g,
    items: navItems.filter((n) => n.group === g),
    color: NAV_COLORS[g],
  }));

  return (
    <div className="min-h-screen bg-background text-foreground flex font-sans text-sm">
      {/* ── Sidebar ── */}
      <aside
        className="w-60 flex-shrink-0 flex flex-col"
        style={{
          background: "linear-gradient(180deg, hsl(220,50%,3%) 0%, hsl(218,48%,4%) 100%)",
          borderRight: `1px solid ${isTestnet ? "rgba(245,158,11,0.12)" : "rgba(0,212,255,0.1)"}`,
          boxShadow: "4px 0 24px rgba(0,0,0,0.4)",
          transition: "border-color 0.4s ease",
        }}
      >
        {/* Logo */}
        <div className="px-4 py-5 flex items-center gap-3" style={{ borderBottom: `1px solid ${isTestnet ? "rgba(245,158,11,0.08)" : "rgba(0,212,255,0.08)"}` }}>
          <div
            className="w-9 h-9 flex items-center justify-center font-black text-lg rounded-lg flex-shrink-0"
            style={{
              background: isTestnet
                ? "linear-gradient(135deg, #F59E0B 0%, #D97706 100%)"
                : "linear-gradient(135deg, #00D4FF 0%, #0080FF 100%)",
              boxShadow: isTestnet
                ? "0 0 20px rgba(245,158,11,0.4), 0 0 40px rgba(245,158,11,0.15)"
                : "0 0 20px rgba(0,212,255,0.4), 0 0 40px rgba(0,212,255,0.15)",
              color: "#000",
              transition: "all 0.4s ease",
            }}
          >
            Z
          </div>
          <div>
            <div
              className="font-black text-base tracking-widest uppercase"
              style={{
                background: isTestnet
                  ? "linear-gradient(90deg, #F59E0B, #D97706)"
                  : "linear-gradient(90deg, #00D4FF, #0080FF)",
                WebkitBackgroundClip: "text",
                WebkitTextFillColor: "transparent",
                transition: "all 0.4s ease",
              }}
            >
              ZBX Chain
            </div>
            <div className="text-[10px] tracking-wider" style={{ color: "rgba(100,116,139,0.9)" }}>
              Zebvix Explorer
            </div>
          </div>
        </div>

        {/* Nav groups */}
        <nav className="flex-1 overflow-y-auto py-3 space-y-4">
          {grouped.map((group) => (
            <div key={group.label}>
              <div className="section-label">{group.label}</div>
              <div className="space-y-0.5 px-2">
                {group.items.map((item) => {
                  const isActive =
                    location === item.path ||
                    (item.path !== "/" && location.startsWith(item.path));
                  return (
                    <Link
                      key={item.path}
                      href={item.path}
                      className={`flex items-center gap-3 px-3 py-2 rounded-md transition-all duration-200 ${
                        isActive
                          ? ""
                          : "hover:bg-white/[0.03] border-l-2 border-transparent"
                      }`}
                      style={
                        isActive
                          ? {
                              background: `linear-gradient(90deg, ${primaryColor}18 0%, ${primaryColor}06 100%)`,
                              borderLeft: `2px solid ${primaryColor}`,
                              boxShadow: `inset 0 0 20px ${primaryColor}08`,
                            }
                          : {}
                      }
                    >
                      <item.icon
                        className="w-4 h-4 flex-shrink-0 transition-all duration-200"
                        style={{ color: isActive ? primaryColor : "rgba(100,116,139,0.8)" }}
                      />
                      <span
                        className="text-[13px] font-medium transition-colors duration-200"
                        style={{ color: isActive ? "#E2E8F0" : "rgba(148,163,184,0.8)" }}
                      >
                        {item.name}
                      </span>
                      {isActive && (
                        <ChevronRight className="w-3 h-3 ml-auto" style={{ color: primaryColor }} />
                      )}
                    </Link>
                  );
                })}
              </div>
            </div>
          ))}
        </nav>

        {/* Chain info footer */}
        <div
          className="px-4 py-3 text-[10px] font-mono space-y-1"
          style={{
            borderTop: `1px solid ${isTestnet ? "rgba(245,158,11,0.08)" : "rgba(0,212,255,0.08)"}`,
            color: "rgba(100,116,139,0.6)",
          }}
        >
          <div className="flex justify-between">
            <span>Chain ID</span>
            <span style={{ color: primaryColor }}>{network.chainId}</span>
          </div>
          <div className="flex justify-between">
            <span>Block</span>
            <span style={{ color: "#4ADE80" }}>#{blockHeight.toLocaleString()}</span>
          </div>
          <div className="flex justify-between">
            <span>Network</span>
            <span style={{ color: isTestnet ? "#F59E0B" : "#4ADE80" }}>
              {isTestnet ? "Testnet" : "Mainnet"}
            </span>
          </div>
        </div>
      </aside>

      {/* ── Main ── */}
      <main className="flex-1 flex flex-col min-w-0 overflow-hidden">
        {/* Testnet warning banner */}
        {isTestnet && (
          <div
            className="flex items-center justify-center gap-2 py-2 text-xs font-bold"
            style={{
              background: "linear-gradient(90deg, rgba(245,158,11,0.15), rgba(245,158,11,0.08), rgba(245,158,11,0.15))",
              borderBottom: "1px solid rgba(245,158,11,0.25)",
              color: "#F59E0B",
            }}
          >
            <FlaskConical className="w-3.5 h-3.5" />
            You are viewing Zebvix Testnet — tokens have no real value
            <FlaskConical className="w-3.5 h-3.5" />
          </div>
        )}

        {/* Top header */}
        <header
          className="h-14 flex items-center justify-between px-6 flex-shrink-0"
          style={{
            borderBottom: `1px solid ${isTestnet ? "rgba(245,158,11,0.08)" : "rgba(0,212,255,0.08)"}`,
            background: "rgba(6,13,26,0.8)",
            backdropFilter: "blur(12px)",
          }}
        >
          {/* Live indicators */}
          <div className="flex items-center gap-5 text-[11px] font-mono">
            <div className="flex items-center gap-2">
              <div className="w-1.5 h-1.5 rounded-full neon-dot" style={{ background: "#4ADE80" }} />
              <span style={{ color: "#4ADE80" }}>
                {isTestnet ? "Testnet Active" : "Mainnet Active"}
              </span>
            </div>
            <div className="h-3.5" style={{ width: "1px", background: `${primaryColor}25` }} />
            <div className="flex items-center gap-1.5">
              <Wifi className="w-3 h-3" style={{ color: "rgba(100,116,139,0.8)" }} />
              <span style={{ color: "rgba(148,163,184,0.8)" }}>ZBX</span>
              {isTestnet ? (
                <span style={{ color: "#F59E0B" }}>Testnet</span>
              ) : (
                <>
                  <span className="font-bold" style={{ color: "#E2E8F0" }}>$1.42</span>
                  <span style={{ color: "#4ADE80" }}>+2.4%</span>
                </>
              )}
            </div>
            <div className="h-3.5" style={{ width: "1px", background: `${primaryColor}25` }} />
            <div className="flex items-center gap-1.5">
              <span style={{ color: "rgba(100,116,139,0.8)" }}>Block</span>
              <span style={{ color: primaryColor }}>#{blockHeight.toLocaleString()}</span>
            </div>
          </div>

          {/* Right side: Network switcher + Search */}
          <div className="flex items-center gap-3">
            {/* Network switcher dropdown */}
            <div ref={dropdownRef} className="relative">
              <button
                onClick={() => setNetworkOpen((o) => !o)}
                className="flex items-center gap-2 px-3 py-1.5 rounded-lg text-[11px] font-bold transition-all duration-200"
                style={{
                  background: network.badgeBg,
                  border: `1px solid ${primaryColor}35`,
                  color: network.badgeText,
                }}
              >
                {isTestnet ? (
                  <FlaskConical className="w-3.5 h-3.5" />
                ) : (
                  <Globe className="w-3.5 h-3.5" />
                )}
                {network.badge}
                <ChevronDown
                  className="w-3 h-3 transition-transform duration-200"
                  style={{ transform: networkOpen ? "rotate(180deg)" : "rotate(0deg)" }}
                />
              </button>

              {networkOpen && (
                <div
                  className="absolute right-0 top-full mt-2 w-56 rounded-xl overflow-hidden z-50"
                  style={{
                    background: "rgba(10,22,40,0.97)",
                    border: "1px solid rgba(0,212,255,0.15)",
                    boxShadow: "0 16px 48px rgba(0,0,0,0.7), 0 0 30px rgba(0,212,255,0.06)",
                    backdropFilter: "blur(16px)",
                  }}
                >
                  <div className="px-3 py-2.5" style={{ borderBottom: "1px solid rgba(0,212,255,0.08)" }}>
                    <p className="text-[10px] font-bold uppercase tracking-wider" style={{ color: "rgba(100,116,139,0.7)" }}>
                      Switch Network
                    </p>
                  </div>
                  {(Object.keys(NETWORKS) as NetworkId[]).map((nid) => {
                    const cfg = NETWORKS[nid];
                    const isSelected = nid === networkId;
                    return (
                      <button
                        key={nid}
                        onClick={() => { setNetwork(nid); setNetworkOpen(false); }}
                        className="w-full flex items-center gap-3 px-3 py-3 text-left transition-all duration-150"
                        style={{
                          background: isSelected ? `${cfg.primary}0F` : "transparent",
                          borderBottom: nid === "mainnet" ? "1px solid rgba(0,212,255,0.06)" : "none",
                        }}
                        onMouseEnter={(e) => { if (!isSelected) (e.currentTarget as HTMLElement).style.background = "rgba(255,255,255,0.03)"; }}
                        onMouseLeave={(e) => { if (!isSelected) (e.currentTarget as HTMLElement).style.background = "transparent"; }}
                      >
                        <div
                          className="w-8 h-8 rounded-lg flex items-center justify-center font-black text-sm flex-shrink-0"
                          style={{
                            background: nid === "mainnet"
                              ? "linear-gradient(135deg, #00D4FF, #0080FF)"
                              : "linear-gradient(135deg, #F59E0B, #D97706)",
                            color: "#000",
                            boxShadow: `0 0 12px ${cfg.primary}40`,
                          }}
                        >
                          {nid === "mainnet" ? "M" : "T"}
                        </div>
                        <div className="flex-1 min-w-0">
                          <div className="text-xs font-bold" style={{ color: isSelected ? cfg.primary : "#E2E8F0" }}>
                            {cfg.label}
                          </div>
                          <div className="text-[10px] font-mono" style={{ color: "rgba(100,116,139,0.7)" }}>
                            Chain ID: {cfg.chainId}
                          </div>
                        </div>
                        {isSelected && (
                          <CheckCircle2 className="w-4 h-4 flex-shrink-0" style={{ color: cfg.primary }} />
                        )}
                      </button>
                    );
                  })}

                  <div className="px-3 py-2" style={{ borderTop: "1px solid rgba(0,212,255,0.06)" }}>
                    <p className="text-[10px]" style={{ color: "rgba(100,116,139,0.5)" }}>
                      Testnet tokens have no real-world value
                    </p>
                  </div>
                </div>
              )}
            </div>

            {/* Search */}
            <div className="relative">
              <Search
                className="w-3.5 h-3.5 absolute left-3 top-1/2 -translate-y-1/2"
                style={{ color: "rgba(100,116,139,0.6)" }}
              />
              <input
                type="text"
                value={searchValue}
                onChange={(e) => setSearchValue(e.target.value)}
                placeholder="Search tx / block / address / PayID"
                className="pl-9 pr-4 py-1.5 text-[11px] font-mono w-68 rounded-md outline-none transition-all duration-200"
                style={{
                  background: `${primaryColor}06`,
                  border: `1px solid ${primaryColor}18`,
                  color: "#E2E8F0",
                  width: "260px",
                }}
                onFocus={(e) => {
                  e.currentTarget.style.border = `1px solid ${primaryColor}55`;
                  e.currentTarget.style.boxShadow = `0 0 12px ${primaryColor}18`;
                }}
                onBlur={(e) => {
                  e.currentTarget.style.border = `1px solid ${primaryColor}18`;
                  e.currentTarget.style.boxShadow = "none";
                }}
              />
            </div>
          </div>
        </header>

        {/* Page content */}
        <div className="flex-1 overflow-y-auto p-6 lg:p-8">
          <div className="max-w-[1600px] mx-auto">{children}</div>
        </div>
      </main>
    </div>
  );
}
