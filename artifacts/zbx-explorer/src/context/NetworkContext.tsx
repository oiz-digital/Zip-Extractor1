import React, { createContext, useContext, useState, useEffect } from "react";
import { setExtraHeaders } from "@workspace/api-client-react";

export type NetworkId = "mainnet" | "testnet";

export interface NetworkConfig {
  id: NetworkId;
  label: string;
  chainId: number;
  primary: string;
  primaryRgb: string;
  badge: string;
  badgeBg: string;
  badgeText: string;
  blockHeightBase: number;
  tpsBase: number;
  supply: string;
  validators: number;
}

export const NETWORKS: Record<NetworkId, NetworkConfig> = {
  mainnet: {
    id: "mainnet",
    label: "Zebvix Mainnet",
    chainId: 8989,
    primary: "#00D4FF",
    primaryRgb: "0, 212, 255",
    badge: "MAINNET",
    badgeBg: "rgba(0,212,255,0.12)",
    badgeText: "#00D4FF",
    blockHeightBase: 4_872_341,
    tpsBase: 700,
    supply: "150,000,000",
    validators: 100,
  },
  testnet: {
    id: "testnet",
    label: "Zebvix Testnet",
    chainId: 8990,
    primary: "#F59E0B",
    primaryRgb: "245, 158, 11",
    badge: "TESTNET",
    badgeBg: "rgba(245,158,11,0.15)",
    badgeText: "#F59E0B",
    blockHeightBase: 1_284_507,
    tpsBase: 120,
    supply: "1,000,000,000",
    validators: 28,
  },
};

interface NetworkContextValue {
  network: NetworkConfig;
  networkId: NetworkId;
  setNetwork: (id: NetworkId) => void;
}

const NetworkContext = createContext<NetworkContextValue>({
  network: NETWORKS.mainnet,
  networkId: "mainnet",
  setNetwork: () => {},
});

export function NetworkProvider({ children }: { children: React.ReactNode }) {
  const [networkId, setNetworkId] = useState<NetworkId>(() => {
    try {
      return (localStorage.getItem("zbx_network") as NetworkId) || "mainnet";
    } catch {
      return "mainnet";
    }
  });

  const network = NETWORKS[networkId];

  function setNetwork(id: NetworkId) {
    setNetworkId(id);
    try {
      localStorage.setItem("zbx_network", id);
    } catch {}
  }

  /* Inject CSS variables and network header whenever network changes */
  useEffect(() => {
    const root = document.documentElement;
    const cfg = NETWORKS[networkId];

    /* Swap the primary HSL token */
    if (networkId === "testnet") {
      root.style.setProperty("--primary", "38 92% 50%");
      root.style.setProperty("--ring", "38 92% 50%");
      root.style.setProperty("--sidebar-primary", "38 92% 50%");
      root.style.setProperty("--sidebar-ring", "38 92% 50%");
    } else {
      root.style.setProperty("--primary", "194 100% 50%");
      root.style.setProperty("--ring", "194 100% 50%");
      root.style.setProperty("--sidebar-primary", "194 100% 50%");
      root.style.setProperty("--sidebar-ring", "194 100% 50%");
    }

    /* data attribute lets CSS target [data-network="testnet"] */
    root.setAttribute("data-network", networkId);

    /* pass network to every API request */
    setExtraHeaders({ "x-zbx-network": networkId });
  }, [networkId]);

  /* Set header on first mount */
  useEffect(() => {
    setExtraHeaders({ "x-zbx-network": networkId });
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  return (
    <NetworkContext.Provider value={{ network, networkId, setNetwork }}>
      {children}
    </NetworkContext.Provider>
  );
}

export function useNetwork() {
  return useContext(NetworkContext);
}
