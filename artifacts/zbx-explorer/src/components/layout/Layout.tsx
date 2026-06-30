import React from "react";
import { Link, useLocation } from "wouter";
import { 
  Activity, 
  Box, 
  ListOrdered, 
  Users, 
  PieChart, 
  Cpu, 
  Link as LinkIcon, 
  Database, 
  Gavel, 
  Gamepad2, 
  Layers
} from "lucide-react";

interface NavItem {
  name: string;
  path: string;
  icon: React.ElementType;
}

const navItems: NavItem[] = [
  { name: "Dashboard", path: "/", icon: Activity },
  { name: "Blocks", path: "/blocks", icon: Box },
  { name: "Transactions", path: "/txs", icon: ListOrdered },
  { name: "Mempool", path: "/mempool", icon: Layers },
  { name: "Validators", path: "/validators", icon: Users },
  { name: "DeFi Hub", path: "/defi", icon: PieChart },
  { name: "On-Chain AI", path: "/ai", icon: Cpu },
  { name: "Cross-Chain", path: "/xcl", icon: LinkIcon },
  { name: "Oracle Prices", path: "/oracle", icon: Database },
  { name: "Governance", path: "/governance", icon: Gavel },
  { name: "NFT & Gaming", path: "/nft", icon: Gamepad2 },
];

export default function Layout({ children }: { children: React.ReactNode }) {
  const [location] = useLocation();

  return (
    <div className="min-h-screen bg-background text-foreground flex flex-col md:flex-row font-mono text-sm">
      {/* Sidebar */}
      <aside className="w-full md:w-64 bg-sidebar border-b md:border-r border-sidebar-border flex-shrink-0">
        <div className="p-4 border-b border-sidebar-border flex items-center gap-3">
          <div className="w-8 h-8 bg-primary text-primary-foreground flex items-center justify-center font-bold text-lg">
            Z
          </div>
          <div>
            <h1 className="font-bold text-base uppercase tracking-wider">Zebvix</h1>
            <p className="text-xs text-muted-foreground">Command Center</p>
          </div>
        </div>
        <nav className="p-2 space-y-1">
          {navItems.map((item) => {
            const isActive = location === item.path || (item.path !== "/" && location.startsWith(item.path));
            return (
              <Link 
                key={item.path} 
                href={item.path}
                className={`flex items-center gap-3 px-3 py-2 transition-colors ${
                  isActive 
                    ? "bg-sidebar-accent text-sidebar-accent-foreground border-l-2 border-primary" 
                    : "text-sidebar-foreground hover:bg-sidebar-accent/50 hover:text-sidebar-accent-foreground border-l-2 border-transparent"
                }`}
              >
                <item.icon className="w-4 h-4" />
                <span>{item.name}</span>
              </Link>
            );
          })}
        </nav>
      </aside>

      {/* Main Content */}
      <main className="flex-1 flex flex-col min-w-0 overflow-hidden">
        <header className="h-14 border-b border-border flex items-center justify-between px-6 bg-card shrink-0">
          <div className="flex items-center gap-4 text-xs text-muted-foreground">
            <div className="flex items-center gap-2">
              <span className="w-2 h-2 rounded-full bg-green-500 animate-pulse"></span>
              Mainnet Active
            </div>
            <div className="hidden sm:block border-l border-border h-4 mx-2"></div>
            <div className="hidden sm:block">ZBX Price: $1.42 <span className="text-green-500">+2.4%</span></div>
          </div>
          <div className="flex items-center gap-4">
            <input 
              type="text" 
              placeholder="Search tx / block / address / ENS"
              className="bg-input border border-border px-3 py-1 text-xs w-64 focus:outline-none focus:border-primary placeholder:text-muted-foreground"
            />
          </div>
        </header>
        <div className="flex-1 overflow-y-auto p-4 md:p-6 lg:p-8">
          <div className="max-w-[1600px] mx-auto">
            {children}
          </div>
        </div>
      </main>
    </div>
  );
}
