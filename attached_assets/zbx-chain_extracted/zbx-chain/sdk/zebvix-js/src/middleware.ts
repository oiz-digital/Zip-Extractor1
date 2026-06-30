/**
 * Middleware and plugin system for zebvix.js.
 * Allows extending ZbxClient with custom behavior (logging, caching, retry, etc.)
 *
 * @example
 * import { ZbxClient, logger, cache, retry } from "zebvix.js";
 *
 * const zbx = new ZbxClient("http://127.0.0.1:8545");
 *
 * // Add built-in middleware
 * zbx.use(logger());           // log all RPC calls
 * zbx.use(cache(30_000));      // cache read-only calls for 30s
 * zbx.use(retry(3));           // retry failed calls 3 times
 *
 * // Custom middleware
 * zbx.use(async (method, params, next) => {
 *   console.log("Before:", method);
 *   const result = await next(method, params);
 *   console.log("After:", method, "→", result);
 *   return result;
 * });
 */

export type MiddlewareFn = (
  method: string,
  params: unknown[],
  next:   (method: string, params: unknown[]) => Promise<unknown>,
) => Promise<unknown>;

/** Built-in: log all RPC calls with timing */
export function logger(prefix = "[zbx]"): MiddlewareFn {
  return async (method, params, next) => {
    const start = Date.now();
    try {
      const result = await next(method, params);
      console.log(`\${prefix} \${method} (\${Date.now() - start}ms)`);
      return result;
    } catch (err) {
      console.error(`\${prefix} \${method} FAILED (\${Date.now() - start}ms):`, err);
      throw err;
    }
  };
}

/** Built-in: cache read-only RPC results (LRU, max 256 entries) */
export function cache(ttlMs = 30_000, maxEntries = 256): MiddlewareFn {
  const READ_ONLY_METHODS = new Set([
    "eth_blockNumber", "eth_chainId", "eth_getBalance", "eth_call",
    "zbx_getBlockByNumber", "zbx_resolvePayId", "zbx_getZusdBalance",
    "zbx_getPriceUSD", "zbx_getPool", "zbx_getNonce",
  ]);
  const store = new Map<string, { value: unknown; expires: number }>();

  return async (method, params, next) => {
    if (!READ_ONLY_METHODS.has(method)) return next(method, params);

    const key     = method + JSON.stringify(params);
    const cached  = store.get(key);
    if (cached && Date.now() < cached.expires) return cached.value;

    const result = await next(method, params);

    // LRU eviction
    if (store.size >= maxEntries) {
      const firstKey = store.keys().next().value;
      if (firstKey) store.delete(firstKey);
    }
    store.set(key, { value: result, expires: Date.now() + ttlMs });
    return result;
  };
}

/** Built-in: retry failed RPC calls with exponential backoff */
export function retry(maxAttempts = 3, baseDelayMs = 500): MiddlewareFn {
  return async (method, params, next) => {
    let lastError: Error;
    for (let attempt = 1; attempt <= maxAttempts; attempt++) {
      try {
        return await next(method, params);
      } catch (err) {
        lastError = err as Error;
        if (attempt < maxAttempts) {
          const delay = baseDelayMs * 2 ** (attempt - 1);
          await new Promise(r => setTimeout(r, delay));
        }
      }
    }
    throw lastError!;
  };
}

/** Built-in: rate limit RPC calls */
export function rateLimit(callsPerSecond = 10): MiddlewareFn {
  const queue: Array<() => void> = [];
  let tokens = callsPerSecond;
  setInterval(() => {
    tokens = Math.min(callsPerSecond, tokens + callsPerSecond);
    while (tokens > 0 && queue.length > 0) {
      tokens--;
      queue.shift()?.();
    }
  }, 1000);

  return async (method, params, next) => {
    if (tokens > 0) {
      tokens--;
      return next(method, params);
    }
    return new Promise((resolve, reject) => {
      queue.push(() => next(method, params).then(resolve, reject));
    });
  };
}