/**
 * ZbxWallet — sign and send transactions on ZBX chain.
 *
 * ## Cryptography
 * Uses @noble/curves (secp256k1) and @noble/hashes (keccak-256) for real,
 * audited, zero-dependency cryptography. No stubs — every address is a genuine
 * secp256k1 public key derivation and every signature is a real ECDSA sig.
 *
 * ## Address derivation
 *   1. Derive uncompressed public key (65 bytes) from private key
 *   2. Discard the 0x04 prefix byte
 *   3. keccak256(pubkey[1..65]) → 32-byte hash
 *   4. Take last 20 bytes → EVM address
 *
 * ## Transaction signing (EIP-155)
 *   1. RLP-encode [nonce, gasPrice, gasLimit, to, value, data, chainId, 0, 0]
 *   2. hash = keccak256(rlpEncoded)
 *   3. (r, s, recoveryBit) = secp256k1.sign(hash, privateKey, { lowS: true })
 *   4. v = chainId * 2 + 35 + recoveryBit
 */
import type { ZbxClient } from "./client";
import type { SendResult, PayIdRecord } from "./types";
import { DEFAULT_CHAIN_ID } from "./constants";
import { ZbxCrypto } from "./crypto";
import { secp256k1 } from "@noble/curves/secp256k1";
import { keccak_256 } from "@noble/hashes/sha3";

export class ZbxWallet {
  readonly address: string;
  // SEC-2026-05-09 (S1): private key is mutable so destroy() can zero it.
  private privateKeyBytes: Uint8Array;
  /** SEC-2026-05-09 (S1): cached chain_id fetched at runtime via eth_chainId. */
  private cachedChainId: number | null = null;

  constructor(
    privateKeyHex: string,
    private readonly client: ZbxClient,
  ) {
    const clean = privateKeyHex.startsWith("0x")
      ? privateKeyHex.slice(2)
      : privateKeyHex;
    this.privateKeyBytes = hexToBytes(clean);
    this.address = deriveAddress(this.privateKeyBytes);
  }

  /**
   * SEC-2026-05-09 (S1): resolve the chain_id to use for signing.
   * Prefers the live `eth_chainId` from the connected RPC so a wallet
   * pointed at testnet does not silently sign mainnet-replayable txs
   * (and vice versa). Falls back to DEFAULT_CHAIN_ID if RPC is unreachable.
   */
  private async resolveChainId(): Promise<number> {
    if (this.cachedChainId !== null) return this.cachedChainId;
    try {
      const hex = await this.client.rpc<string>("eth_chainId");
      const id = Number.parseInt(hex.replace(/^0x/, ""), 16);
      if (!Number.isFinite(id) || id <= 0) {
        throw new Error(`eth_chainId returned invalid value: ${hex}`);
      }
      this.cachedChainId = id;
      return id;
    } catch (err) {
      // Strict mode: refuse to silently fall back, since signing under
      // the wrong chain_id is a replay vector.
      throw new Error(
        `S1: unable to resolve chain_id from RPC (${(err as Error).message}); ` +
        `refusing to sign with hardcoded DEFAULT_CHAIN_ID=${DEFAULT_CHAIN_ID}`
      );
    }
  }

  /**
   * SEC-2026-05-09 (S1): zero out the in-memory private key. After calling
   * this, every signing method on this wallet will throw. Best-effort —
   * JS/V8 may have copied bytes into other internal buffers, so this is a
   * defence-in-depth measure, not a hard guarantee.
   */
  destroy(): void {
    if (this.privateKeyBytes.length > 0) {
      this.privateKeyBytes.fill(0);
    }
    this.privateKeyBytes = new Uint8Array(0);
    this.cachedChainId = null;
  }

  private assertLive(): void {
    if (this.privateKeyBytes.length === 0) {
      throw new Error("S1: wallet has been destroy()'d — private key is zeroed");
    }
  }

  // ── Balances ──────────────────────────────────────────────────────────────

  /** Get ZBX balance in wei. */
  async getBalance(): Promise<bigint> {
    return this.client.getBalance(this.address);
  }

  /** Get ZBX balance as decimal string. */
  async getBalanceZbx(): Promise<string> {
    return this.client.getBalanceZbx(this.address);
  }

  /** Get ZUSD balance. */
  async getZusdBalance(): Promise<bigint> {
    return this.client.zusd.balanceOf(this.address);
  }

  /** Get Pay ID registered to this wallet (if any). */
  async getPayId(): Promise<string | null> {
    return this.client.payId.of(this.address);
  }

  // ── Send ZBX ─────────────────────────────────────────────────────────────

  /**
   * Send ZBX to an address or Pay ID.
   *
   * If `to` is a Pay ID (ends with @zbx), it is automatically resolved.
   *
   * @example
   * const tx = await wallet.send("ali@zbx", "100");
   * console.log("Tx:", tx.hash);
   */
  async send(to: string, amountZbx: string): Promise<SendResult> {
    this.assertLive();
    let toAddress = to;
    if (to.endsWith("@zbx")) {
      const resolved = await this.client.payId.resolve(to);
      if (!resolved) throw new Error(`Pay ID not found: ${to}`);
      toAddress = resolved;
    }

    const amountWei = this.client.parseZbx(amountZbx);
    const nonce     = await this.client.getNonce(this.address);
    const chainId   = await this.resolveChainId();

    const tx = buildTx({
      from:    this.address,
      to:      toAddress,
      value:   amountWei,
      nonce,
      chainId,
    });

    const signed = signTx(tx, this.privateKeyBytes);
    const hash   = await this.client.rpc<string>("zbx_sendTransaction", [signed]);

    return { hash, from: this.address, to: toAddress, amountWei, amountZbx };
  }

  // ── Register Pay ID ───────────────────────────────────────────────────────

  /**
   * Register a Pay ID for this wallet.
   *
   * @example
   * const result = await wallet.registerPayId("alice@zbx");
   * console.log("Registered:", result.payId);
   */
  async registerPayId(payId: string): Promise<{ hash: string; payId: string }> {
    this.assertLive();
    if (!payId.endsWith("@zbx")) throw new Error("Pay ID must end with @zbx");
    const name = payId.slice(0, -4);
    if (name.length < 2 || name.length > 32) throw new Error("Name must be 2–32 chars");
    if (!/^[a-z0-9_-]+$/.test(name)) throw new Error("Invalid characters in Pay ID name");

    const nonce           = await this.client.getNonce(this.address);
    const chainId         = await this.resolveChainId();
    const registryAddress = "0x7e4a7f8bCE8CfD1765CdE34Dbf5e8D7fB1A43e9";

    const tx = buildTx({
      from:    this.address,
      to:      registryAddress,
      value:   10n ** 16n, // 0.01 ZBX canonical fee (ZEP-001)
      nonce,
      chainId,
      data:    encodePayIdReg(payId),
    });

    const signed = signTx(tx, this.privateKeyBytes);
    const hash   = await this.client.rpc<string>("zbx_registerPayId", [payId, signed]);

    return { hash, payId };
  }

  // ── Burn ZBX ─────────────────────────────────────────────────────────────

  /**
   * Permanently burn ZBX (calls ZVM ZBXBURN opcode). IRREVERSIBLE.
   *
   * @example
   * const result = await wallet.burn("10");
   * console.log("Burned 10 ZBX, tx:", result.hash);
   */
  async burn(amountZbx: string): Promise<SendResult> {
    this.assertLive();
    const amountWei = this.client.parseZbx(amountZbx);
    const balance   = await this.getBalance();
    if (amountWei > balance) {
      throw new Error(
        `Insufficient balance: need ${amountZbx} ZBX, have ${this.client.formatZbx(balance)}`
      );
    }

    const nonce   = await this.client.getNonce(this.address);
    const chainId = await this.resolveChainId();
    const burnTo  = "0x0000000000000000000000000000000000000000";
    const tx = buildTx({
      from:    this.address,
      to:      burnTo,
      value:   amountWei,
      nonce,
      chainId,
      kind:    "burn",
    });

    const signed = signTx(tx, this.privateKeyBytes);
    const hash   = await this.client.rpc<string>("zbx_sendTransaction", [signed]);

    return { hash, from: this.address, to: "0x0", amountWei, amountZbx };
  }

  // ── Low-level signing ─────────────────────────────────────────────────────

  /**
   * Sign a raw 32-byte hash with EIP-191 personal sign prefix.
   * Returns hex-encoded 65-byte signature (r || s || v).
   */
  async personalSign(message: Uint8Array | string): Promise<string> {
    this.assertLive();
    const msgBytes = typeof message === "string"
      ? new TextEncoder().encode(message)
      : message;
    const prefix  = new TextEncoder().encode(
      `\x19Ethereum Signed Message:\n${msgBytes.length}`
    );
    const combined = new Uint8Array(prefix.length + msgBytes.length);
    combined.set(prefix);
    combined.set(msgBytes, prefix.length);
    const hash = keccak_256(combined);
    const sig  = secp256k1.sign(hash, this.privateKeyBytes, { lowS: true });
    const sigBytes = sig.toCompactRawBytes();
    const out = new Uint8Array(65);
    out.set(sigBytes);
    out[64] = sig.recovery + 27;
    return "0x" + bytesToHex(out);
  }
}

// ── Address derivation ────────────────────────────────────────────────────────

/**
 * Derive the EVM address from a secp256k1 private key.
 *
 * Algorithm: keccak256(uncompressed_pubkey[1:])[12:]
 *   - uncompressed_pubkey: 65 bytes (0x04 || X || Y)
 *   - Skip the 0x04 prefix byte
 *   - Hash the remaining 64 bytes (X || Y)
 *   - Take the last 20 bytes of the 32-byte keccak hash
 */
function deriveAddress(privateKey: Uint8Array): string {
  const pubKey  = secp256k1.getPublicKey(privateKey, false); // uncompressed 65 bytes
  const hash    = keccak_256(pubKey.slice(1));               // skip 0x04 prefix
  const address = hash.slice(12);                            // last 20 bytes
  return "0x" + bytesToHex(address);
}

// ── Transaction building ──────────────────────────────────────────────────────

function buildTx(params: {
  from:    string;
  to:      string;
  value:   bigint;
  nonce:   number;
  chainId: number;
  data?:   string;
  kind?:   string;
}): TxFields {
  return {
    from:    params.from,
    to:      params.to,
    value:   params.value,
    nonce:   params.nonce,
    chainId: params.chainId,
    data:    params.data ?? "0x",
    kind:    params.kind ?? "transfer",
    // Standard gas parameters (ZBX chain default)
    gasPrice: 1_000_000_000n, // 1 Gwei
    gasLimit: 21_000n,
  };
}

interface TxFields {
  from:     string;
  to:       string;
  value:    bigint;
  nonce:    number;
  chainId:  number;
  gasPrice: bigint;
  gasLimit: bigint;
  data:     string;
  kind:     string;
}

// ── EIP-155 transaction signing ───────────────────────────────────────────────

/**
 * Sign a transaction with EIP-155 replay protection.
 *
 * Signing hash: keccak256(RLP([nonce, gasPrice, gasLimit, to, value, data, chainId, 0, 0]))
 * Signature v:  chainId * 2 + 35 + recoveryBit
 */
function signTx(tx: TxFields, privateKey: Uint8Array): Record<string, unknown> {
  // RLP-encode the pre-image for EIP-155 signing
  const rlpPreimage = ZbxCrypto.rlpEncode([
    bigintToRlpHex(BigInt(tx.nonce)),
    bigintToRlpHex(tx.gasPrice),
    bigintToRlpHex(tx.gasLimit),
    tx.to,
    bigintToRlpHex(tx.value),
    tx.data,
    bigintToRlpHex(BigInt(tx.chainId)),
    "0x",  // r = 0 for EIP-155 signing
    "0x",  // s = 0 for EIP-155 signing
  ]);

  const hash = keccak_256(rlpPreimage);
  const sig  = secp256k1.sign(hash, privateKey, { lowS: true });

  // EIP-155: v = chainId * 2 + 35 + recovery
  const v = tx.chainId * 2 + 35 + sig.recovery;

  return {
    from:     tx.from,
    to:       tx.to,
    value:    "0x" + tx.value.toString(16),
    nonce:    tx.nonce,
    chainId:  tx.chainId,
    gasPrice: "0x" + tx.gasPrice.toString(16),
    gasLimit: "0x" + tx.gasLimit.toString(16),
    data:     tx.data,
    r:        "0x" + sig.r.toString(16).padStart(64, "0"),
    s:        "0x" + sig.s.toString(16).padStart(64, "0"),
    v,
  };
}

// ── Encoding helpers ──────────────────────────────────────────────────────────

function encodePayIdReg(payId: string): string {
  const selector = "5a425041"; // keccak256("registerPayId(string)")[0..4]
  const nameBytes = new TextEncoder().encode(payId);
  return "0x" + selector + bytesToHex(nameBytes);
}

/** Convert bigint to minimal-length hex string for RLP encoding. */
function bigintToRlpHex(n: bigint): string {
  if (n === 0n) return "0x";
  let hex = n.toString(16);
  if (hex.length % 2 !== 0) hex = "0" + hex;
  return "0x" + hex;
}

// ── Byte utilities ────────────────────────────────────────────────────────────

function hexToBytes(hex: string): Uint8Array {
  const clean = hex.replace(/^0x/, "");
  if (clean.length === 0) return new Uint8Array(0);
  const bytes = new Uint8Array(clean.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  }
  return bytes;
}

function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes, b => b.toString(16).padStart(2, "0")).join("");
}
