/**
 * React + @zebvix/ethers — dApp example
 *
 * Shows how to connect MetaMask to ZBX chain and use Pay IDs in a React dApp.
 * Install: npm install @zebvix/ethers react
 */
import { useState, useEffect } from "react";
import { BrowserProvider, formatEther } from "ethers";
import { ZbxProvider, ZbxWallet, PayId, ZUSD, zbxMainnet } from "@zebvix/ethers";

/** Add ZBX chain to MetaMask */
async function addZbxToMetaMask() {
  await window.ethereum.request({
    method: "wallet_addEthereumChain",
    params: [{
      chainId:           "0x231D", // 8989 mainnet in hex (testnet+devnet: "0x231E" / 8990)
      chainName:         "Zebvix Chain",
      nativeCurrency:    { name: "ZBX", symbol: "ZBX", decimals: 18 },
      rpcUrls:           ["https://rpc.zebvix.io"],
      blockExplorerUrls: ["https://explorer.zebvix.io"],
    }],
  });
}

export function ZbxDApp() {
  const [address, setAddress]   = useState<string | null>(null);
  const [zbxBal, setZbxBal]     = useState<string>("0");
  const [zusdBal, setZusdBal]   = useState<string>("0");
  const [payId, setPayId]       = useState<string | null>(null);
  const [sendTo, setSendTo]     = useState("");
  const [sendAmt, setSendAmt]   = useState("");
  const [status, setStatus]     = useState("");
  const [price, setPrice]       = useState<string>("...");

  const provider = new ZbxProvider(zbxMainnet.rpc);

  // Connect MetaMask
  async function connect() {
    await addZbxToMetaMask();
    const browserProvider = new BrowserProvider(window.ethereum);
    const signer = await browserProvider.getSigner();
    const addr = await signer.getAddress();
    setAddress(addr);

    // Load balances
    const zbx  = await browserProvider.getBalance(addr);
    const zusd = await provider.zbx.zusdBalance(addr);
    const pid  = await provider.zbx.payIdOf(addr);
    const priceInfo = await provider.zbx.price();

    setZbxBal(formatEther(zbx));
    setZusdBal(ZUSD.format(zusd));
    setPayId(pid?.payId ?? null);
    setPrice(priceInfo.zbxUsd);
  }

  // Send ZBX to a Pay ID or address
  async function send() {
    if (!address || !sendTo || !sendAmt) return;
    setStatus("Resolving...");

    try {
      // Resolve Pay ID if needed
      let toAddr = sendTo;
      if (PayId.isPayId(sendTo)) {
        const resolved = await PayId.resolve(sendTo, provider);
        if (!resolved) { setStatus("Pay ID not found"); return; }
        toAddr = resolved;
        setStatus(`Resolved \${sendTo} → \${toAddr.slice(0, 10)}...`);
      }

      setStatus("Sending...");
      const browserProvider = new BrowserProvider(window.ethereum);
      const signer = await browserProvider.getSigner();

      const tx = await signer.sendTransaction({
        to:    toAddr,
        value: BigInt(parseFloat(sendAmt) * 1e18),
      });
      setStatus(`Sent! Tx: \${tx.hash.slice(0, 16)}...`);
      await tx.wait();
      setStatus(`Confirmed!`);
    } catch (e: any) {
      setStatus(`Error: \${e.message}`);
    }
  }

  return (
    <div style={{ fontFamily: "monospace", padding: 24 }}>
      <h1>Zebvix dApp</h1>
      <p>ZBX Price: <strong>\${price} USD</strong></p>

      {!address ? (
        <button onClick={connect}>Connect MetaMask</button>
      ) : (
        <div>
          <p>Address: {address}</p>
          {payId && <p>Pay ID: <strong>{payId}</strong></p>}
          <p>ZBX Balance: <strong>{zbxBal} ZBX</strong></p>
          <p>ZUSD Balance: <strong>{zusdBal} ZUSD</strong></p>

          <div style={{ marginTop: 16 }}>
            <input
              placeholder="To (address or ali@zbx)"
              value={sendTo}
              onChange={e => setSendTo(e.target.value)}
              style={{ width: 300, marginRight: 8 }}
            />
            <input
              placeholder="Amount ZBX"
              value={sendAmt}
              onChange={e => setSendAmt(e.target.value)}
              style={{ width: 100, marginRight: 8 }}
            />
            <button onClick={send}>Send ZBX</button>
          </div>

          {status && <p style={{ color: "#888" }}>{status}</p>}
        </div>
      )}
    </div>
  );
}