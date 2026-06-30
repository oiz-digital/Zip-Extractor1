/**
 * ZbxSubscriber — real-time event subscriptions over WebSocket.
 *
 * @example
 * import { ZbxSubscriber } from "zebvix.js";
 *
 * const sub = new ZbxSubscriber("wss://ws.zebvix.io");
 *
 * // New blocks
 * sub.onBlock(block => console.log("New block:", block.height));
 *
 * // Pending transactions
 * sub.onPendingTx(tx => console.log("Pending:", tx.hash));
 *
 * // Address activity (send or receive)
 * sub.onAddress("0x742d35...", tx => console.log("Activity:", tx));
 *
 * // Pay ID activity
 * sub.onPayId("ali@zbx", tx => console.log("Pay ID tx:", tx));
 *
 * // ZVM logs from a contract
 * sub.onZvmLog("0x742d35...", log => console.log("ZVM log:", log));
 *
 * // Cleanup
 * sub.unsubscribe();
 */
import type { BlockInfo, TxInfo } from "./types";

export type BlockCallback   = (block: BlockInfo) => void;
export type TxCallback      = (tx: TxInfo)       => void;
export type LogCallback     = (log: ZvmLogEntry)  => void;

export interface ZvmLogEntry {
  contractAddress: string;
  blockHeight:     number;
  txHash:          string;
  key:             string;
  value:           string;
}

export class ZbxSubscriber {
  private ws:      WebSocket | null = null;
  private subs:    Map<string, Set<Function>> = new Map();
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private closed:  boolean = false;

  constructor(private readonly wsUrl: string) {
    this.connect();
  }

  private connect(): void {
    if (this.closed) return;
    this.ws = new WebSocket(this.wsUrl);

    this.ws.onopen = () => {
      // Re-register all active subscriptions after reconnect
      for (const topic of this.subs.keys()) {
        this.ws!.send(JSON.stringify({ jsonrpc: "2.0", id: 1, method: "zbx_subscribe", params: [topic] }));
      }
    };

    this.ws.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data as string) as {
          method?: string;
          params?: { subscription?: string; result?: unknown };
        };
        if (msg.method === "zbx_subscription" && msg.params) {
          const { subscription, result } = msg.params;
          const handlers = this.subs.get(subscription ?? "");
          handlers?.forEach(fn => fn(result));
        }
      } catch { /* ignore parse errors */ }
    };

    this.ws.onclose = () => {
      if (!this.closed) {
        // Exponential backoff reconnect
        this.retryTimer = setTimeout(() => this.connect(), 3000);
      }
    };

    this.ws.onerror = () => {
      this.ws?.close();
    };
  }

  private subscribe(topic: string, callback: Function): () => void {
    if (!this.subs.has(topic)) {
      this.subs.set(topic, new Set());
      if (this.ws?.readyState === WebSocket.OPEN) {
        this.ws.send(JSON.stringify({ jsonrpc: "2.0", id: 1, method: "zbx_subscribe", params: [topic] }));
      }
    }
    this.subs.get(topic)!.add(callback);
    return () => {
      this.subs.get(topic)?.delete(callback);
      if (this.subs.get(topic)?.size === 0) {
        this.subs.delete(topic);
        if (this.ws?.readyState === WebSocket.OPEN) {
          this.ws.send(JSON.stringify({ jsonrpc: "2.0", id: 1, method: "zbx_unsubscribe", params: [topic] }));
        }
      }
    };
  }

  /**
   * Subscribe to new blocks.
   * Returns unsubscribe function.
   *
   * @example
   * const unsub = sub.onBlock(b => console.log("#" + b.height, b.hash));
   * setTimeout(unsub, 60_000); // stop after 60s
   */
  onBlock(callback: BlockCallback): () => void {
    return this.subscribe("newBlocks", callback as Function);
  }

  /**
   * Subscribe to pending (mempool) transactions.
   * Returns unsubscribe function.
   *
   * @example
   * sub.onPendingTx(tx => console.log("Pending:", tx.hash, tx.from, "→", tx.to));
   */
  onPendingTx(callback: TxCallback): () => void {
    return this.subscribe("pendingTransactions", callback as Function);
  }

  /**
   * Subscribe to all transactions involving a specific address.
   *
   * @example
   * sub.onAddress("0x742d35...", tx => {
   *   if (tx.to === myAddr) console.log("Received", tx.value, "ZBX");
   * });
   */
  onAddress(address: string, callback: TxCallback): () => void {
    return this.subscribe(`address:\${address.toLowerCase()}`, callback as Function);
  }

  /**
   * Subscribe to all transactions involving a Pay ID.
   *
   * @example
   * sub.onPayId("ali@zbx", tx => console.log("Pay ID tx:", tx.hash));
   */
  onPayId(payId: string, callback: TxCallback): () => void {
    return this.subscribe(`payid:\${payId}`, callback as Function);
  }

  /**
   * Subscribe to ZVM log entries (ZVMLOG opcode) from a contract.
   *
   * @example
   * sub.onZvmLog("0x742d35...", log => {
   *   console.log(`ZVM log from \${log.contractAddress}: \${log.key}=\${log.value}`);
   * });
   */
  onZvmLog(contractAddress: string, callback: LogCallback): () => void {
    return this.subscribe(`zvmlog:\${contractAddress.toLowerCase()}`, callback as Function);
  }

  /**
   * Subscribe to ZUSD transfers.
   *
   * @example
   * sub.onZusdTransfer(tx => console.log("ZUSD moved:", tx.value));
   */
  onZusdTransfer(callback: TxCallback): () => void {
    return this.subscribe("zusdTransfer", callback as Function);
  }

  /**
   * Subscribe to Pay ID registrations (any new ali@zbx registrations).
   *
   * @example
   * sub.onPayIdRegister(info => console.log("New Pay ID:", info.payId));
   */
  onPayIdRegister(callback: (info: { payId: string; address: string; blockHeight: number }) => void): () => void {
    return this.subscribe("payIdRegister", callback as Function);
  }

  /**
   * Subscribe to ZBX burns (ZBXBURN opcode).
   *
   * @example
   * sub.onBurn(event => console.log("Burned:", event.amountZbx, "ZBX from", event.from));
   */
  onBurn(callback: (event: { from: string; amountZbx: string; txHash: string }) => void): () => void {
    return this.subscribe("zbxBurn", callback as Function);
  }

  /** Stop all subscriptions and close WebSocket. */
  unsubscribe(): void {
    this.closed = true;
    if (this.retryTimer) clearTimeout(this.retryTimer);
    this.ws?.close();
    this.subs.clear();
  }

  /** Check if WebSocket is currently connected. */
  get connected(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }
}