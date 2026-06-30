import React, { useState, useEffect } from "react";
import { Link, useLocation } from "wouter";
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
} from "lucide-react";

interface NavItem {
  name: string;
  path: string;
  icon: React.ElementType;
  color: string;
  group?: string;
}

const navItems: NavItem[] = [
  { name: "Dashboard", path: "/", icon: LayoutDashboard, color: "#00D4FF", group: "Network" },
  { name: "Blocks", path: "/blocks", icon: Blocks, color: "#00D4FF", group: "Network" },
  { name: "Transactions", path: "/txs", icon: ArrowLeftRight, color: "#00D4FF", group: "Network" },
  { name: "Mempool", path: "/mempool", icon: Clock, color: "#FCD34D", group: "Network" },
  { name: "Validators", path: "/validators", icon: Users, color: "#4ADE80", group: "Staking" },
  { name: "DeFi Hub", path: "/defi", icon: TrendingUp, color: "#4ADE80", group: "Ecosystem" },
  { name: "On-Chain AI", path: "/ai", icon: Brain, color: "#A78BFA", group: "Ecosystem" },
  { name: "Cross-Chain", path: "/xcl", icon: GitBranch, color: "#67E8F9", group: "Ecosystem" },
  { name: "Oracle Prices", path: "/oracle", icon: CircleDot, color: "#FCD34D", group: "Ecosystem" },
  { name: "Governance", path: "/governance", icon: Vote, color: "#FB923C", group: "Governance" },
  { name: "NFT & Gaming", path: "/nft", icon: Gamepad2, color: "#A78BFA", group: "Governance" },
];

const groupOrder = ["Network", "Staking", "Ecosystem", "Governance"];

export default function Layout({ children }: { children: React.ReactNode }) {
  const [location] = useLocation();
  const [searchValue, setSearchValue] = useState("");
  const [blockHeight, setBlockHeight] = useState(4872371);
  const [price] = useState({ value: 1.42, change: 2.4 });

  useEffect(() => {
    const iv = setInterval(() => setBlockHeight((h) => h + 1), 5000);
    return () => clearInterval(iv);
  }, []);

  const grouped = groupOrder.map((g) => ({
    label: g,
    items: navItems.filter((n) => n.group === g),
  }));

  return (
    <div className="min-h-screen bg-background text-foreground flex font-sans text-sm">
      {/* Sidebar */}
      <aside
        className="w-60 flex-shrink-0 flex flex-col"
        style={{
          background: "linear-gradient(180deg, hsl(220,50%,3%) 0%, hsl(218,48%,4%) 100%)",
          borderRight: "1px solid rgba(0,212,255,0.1)",
          boxShadow: "4px 0 24px rgba(0,0,0,0.4)",
        }}
      >
        {/* Logo */}
        <div className="px-4 py-5 flex items-center gap-3" style={{ borderBottom: "1px solid rgba(0,212,255,0.08)" }}>
          <div
            className="w-9 h-9 flex items-center justify-center font-black text-lg rounded-lg flex-shrink-0"
            style={{
              background: "linear-gradient(135deg, #00D4FF 0%, #0080FF 100%)",
              boxShadow: "0 0 20px rgba(0,212,255,0.4), 0 0 40px rgba(0,212,255,0.15)",
              color: "#000",
            }}
          >
            Z
          </div>
          <div>
            <div
              className="font-black text-base tracking-widest uppercase"
              style={{
                background: "linear-gradient(90deg, #00D4FF, #0080FF)",
                WebkitBackgroundClip: "text",
                WebkitTextFillColor: "transparent",
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
                      className={`flex items-center gap-3 px-3 py-2 rounded-md transition-all duration-200 group ${
                        isActive ? "nav-active" : "hover:bg-white/[0.03] border-l-2 border-transparent"
                      }`}
                    >
                      <item.icon
                        className="w-4 h-4 flex-shrink-0 transition-all duration-200"
                        style={{ color: isActive ? item.color : "rgba(100,116,139,0.8)" }}
                      />
                      <span
                        className="text-[13px] font-medium transition-colors duration-200"
                        style={{
                          color: isActive ? "#E2E8F0" : "rgba(148,163,184,0.8)",
                        }}
                      >
                        {item.name}
                      </span>
                      {isActive && (
                        <ChevronRight className="w-3 h-3 ml-auto" style={{ color: item.color }} />
                      )}
                    </Link>
                  );
                })}
              </div>
            </div>
          ))}
        </nav>

        {/* Footer chain info */}
        <div
          className="px-4 py-3 text-[10px] font-mono space-y-1"
          style={{
            borderTop: "1px solid rgba(0,212,255,0.08)",
            color: "rgba(100,116,139,0.6)",
          }}
        >
          <div className="flex justify-between">
            <span>Chain ID</span>
            <span style={{ color: "#00D4FF" }}>8989</span>
          </div>
          <div className="flex justify-between">
            <span>Block</span>
            <span style={{ color: "#4ADE80" }}>#{blockHeight.toLocaleString()}</span>
          </div>
          <div className="flex justify-between">
            <span>Network</span>
            <span style={{ color: "#4ADE80" }}>Mainnet</span>
          </div>
        </div>
      </aside>

      {/* Main */}
      <main className="flex-1 flex flex-col min-w-0 overflow-hidden">
        {/* Top header */}
        <header
          className="h-14 flex items-center justify-between px-6 flex-shrink-0"
          style={{
            borderBottom: "1px solid rgba(0,212,255,0.08)",
            background: "rgba(6,13,26,0.8)",
            backdropFilter: "blur(12px)",
          }}
        >
          {/* Live indicators */}
          <div className="flex items-center gap-5 text-[11px] font-mono">
            <div className="flex items-center gap-2">
              <div
                className="w-1.5 h-1.5 rounded-full neon-dot"
                style={{ background: "#4ADE80" }}
              />
              <span style={{ color: "#4ADE80" }}>Mainnet Active</span>
            </div>
            <div
              className="h-3.5"
              style={{ width: "1px", background: "rgba(0,212,255,0.15)" }}
            />
            <div className="flex items-center gap-1.5">
              <Wifi className="w-3 h-3" style={{ color: "rgba(100,116,139,0.8)" }} />
              <span style={{ color: "rgba(148,163,184,0.8)" }}>ZBX</span>
              <span className="font-bold" style={{ color: "#E2E8F0" }}>${price.value.toFixed(2)}</span>
              <span style={{ color: "#4ADE80" }}>+{price.change}%</span>
            </div>
            <div
              className="h-3.5"
              style={{ width: "1px", background: "rgba(0,212,255,0.15)" }}
            />
            <div className="flex items-center gap-1.5">
              <span style={{ color: "rgba(100,116,139,0.8)" }}>Block</span>
              <span style={{ color: "#00D4FF" }}>#{blockHeight.toLocaleString()}</span>
            </div>
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
              className="pl-9 pr-4 py-1.5 text-[11px] font-mono w-72 rounded-md outline-none transition-all duration-200"
              style={{
                background: "rgba(0,212,255,0.04)",
                border: "1px solid rgba(0,212,255,0.12)",
                color: "#E2E8F0",
              }}
              onFocus={(e) => {
                e.currentTarget.style.border = "1px solid rgba(0,212,255,0.4)";
                e.currentTarget.style.boxShadow = "0 0 12px rgba(0,212,255,0.1)";
              }}
              onBlur={(e) => {
                e.currentTarget.style.border = "1px solid rgba(0,212,255,0.12)";
                e.currentTarget.style.boxShadow = "none";
              }}
            />
          </div>
        </header>

        {/* Page content */}
        <div
          className="flex-1 overflow-y-auto p-6 lg:p-8"
        >
          <div className="max-w-[1600px] mx-auto">
            {children}
          </div>
        </div>
      </main>
    </div>
  );
}
