/**
 * ZbxBatch — send multiple RPC calls in a single HTTP request.
 * Reduces latency significantly when loading many data points at once.
 *
 * @example
 * import { ZbxBatch } from "zebvix.js";
 *
 * const batch = new ZbxBatch(client);
 *
 * // Queue calls
 * const balanceCall  = batch.getBalance("0xAddr1...");
 * const balanceCall2 = batch.getBalance("0xAddr2...");
 * const priceCall    = batch.price();
 * const blockCall    = batch.blockNumber();
 *
 * // Execute all in ONE request
 * await batch.execute();
 *
 * // Get results
 * console.log("Balance 1:", await balanceCall);
 * console.log("Balance 2:", await balanceCall2);
 * console.log("Price:", await priceCall);
 * console.log("Block:", await blockCall);
 */
import type { ZbxClient } from "./client";

interface BatchCall {
  id:      number;
  method:  string;
  params:  unknown[];
  resolve: (value: unknown) => void;
  reject:  (error: Error) => void;
}

type Deferred<T> = Promise<T> & { __batchId: number };

export class ZbxBatch {
  private calls:   BatchCall[] = [];
  private nextId = 1;

  constructor(private readonly client: ZbxClient) {}

  /** Queue a raw RPC call. Returns a promise for the result. */
  call<T = unknown>(method: string, params: unknown[] = []): Promise<T> {
    let res!: (v: T) => void, rej!: (e: Error) => void;
    const promise = new Promise<T>((resolve, reject) => { res = resolve; rej = reject; });
    this.calls.push({ id: this.nextId++, method, params, resolve: res as (v: unknown) => void, reject: rej });
    return promise;
  }

  /** Queue eth_getBalance */
  getBalance(address: string):  Promise<bigint> {
    return this.call<string>("eth_getBalance", [address, "latest"]).then(BigInt);
  }

  /** Queue zbx_getZusdBalance */
  zusdBalance(address: string): Promise<bigint> {
    return this.call<string>("zbx_getZusdBalance", [address]).then(r => BigInt(r || "0"));
  }

  /** Queue eth_blockNumber */
  blockNumber(): Promise<number> {
    return this.call<string>("eth_blockNumber").then(h => parseInt(h, 16));
  }

  /** Queue zbx_getPriceUSD */
  price(): Promise<{ zbxUsd: string }> {
    return this.call<{ zbxUsd: string }>("zbx_getPriceUSD");
  }

  /** Queue zbx_resolvePayId */
  resolvePayId(payId: string): Promise<string | null> {
    return this.call<string>("zbx_resolvePayId", [payId]).then(addr =>
      addr && addr !== "0x0000000000000000000000000000000000000000" ? addr : null
    );
  }

  /** Queue zbx_getTransaction */
  getTransaction(hash: string): Promise<unknown> {
    return this.call("zbx_getTransaction", [hash]);
  }

  /**
   * Execute all queued calls in ONE HTTP request.
   * Must be called after queuing calls.
   *
   * @example
   * const b1 = batch.getBalance("0xAddr1...");
   * const b2 = batch.getBalance("0xAddr2...");
   * await batch.execute();
   * console.log(await b1, await b2);
   */
  async execute(): Promise<void> {
    if (this.calls.length === 0) return;

    const pendingCalls = [...this.calls];
    this.calls = [];
    this.nextId = 1;

    const body = pendingCalls.map(c => ({
      jsonrpc: "2.0",
      id:      c.id,
      method:  c.method,
      params:  c.params,
    }));

    const res  = await fetch((this.client as any).rpcUrl, {
      method:  "POST",
      headers: { "Content-Type": "application/json" },
      body:    JSON.stringify(body),
    });

    if (!res.ok) {
      const err = new Error(`Batch HTTP \${res.status}`);
      pendingCalls.forEach(c => c.reject(err));
      return;
    }

    const results = await res.json() as Array<{ id: number; result?: unknown; error?: { message: string } }>;
    const byId    = new Map(results.map(r => [r.id, r]));

    for (const call of pendingCalls) {
      const result = byId.get(call.id);
      if (!result)             { call.reject(new Error("Missing batch response")); continue; }
      if (result.error)        { call.reject(new Error(result.error.message)); continue; }
      call.resolve(result.result);
    }
  }
}