/**
 * ZbxCrypto — cryptographic utilities for ZBX chain.
 *
 * Uses @noble/hashes for keccak256 (same library used throughout the SDK).
 * Uses Web Crypto API for random key generation.
 */
import { keccak_256 } from "@noble/hashes/sha3";

export const ZbxCrypto = {

  /**
   * Generate a new random private key (32 bytes, hex).
   *
   * @example
   * const key = ZbxCrypto.generateKey();
   * // "a1b2c3d4..." (64 hex chars)
   */
  generateKey(): string {
    const bytes = new Uint8Array(32);
    crypto.getRandomValues(bytes);
    return Array.from(bytes, b => b.toString(16).padStart(2, "0")).join("");
  },

  /**
   * Keccak-256 hash (used for address derivation, tx hashing, EIP-191 signing).
   *
   * Uses @noble/hashes/sha3 — the same implementation used by ethers.js and
   * viem. Synchronous (keccak_256 does not require Web Crypto).
   *
   * @example
   * const hash = await ZbxCrypto.keccak256(new Uint8Array([1, 2, 3]));
   * // "f1885..."
   */
  async keccak256(input: Uint8Array): Promise<string> {
    const hash = keccak_256(input);
    return Array.from(hash, b => b.toString(16).padStart(2, "0")).join("");
  },

  /**
   * Derive ZBX address from a public key (Ethereum-compatible).
   * ZBX uses the same address format: keccak256(pubkey)[12:]
   *
   * @param pubkeyHex 65-byte uncompressed SEC1 public key as hex string
   */
  async pubkeyToAddress(pubkeyHex: string): Promise<string> {
    const bytes = hexToBytes(pubkeyHex);
    const hash  = await ZbxCrypto.keccak256(bytes);
    return "0x" + hash.slice(-40);
  },

  /**
   * Encode a value to RLP (Recursive Length Prefix).
   * Used for transaction serialization.
   */
  rlpEncode(value: string | Uint8Array | string[]): Uint8Array {
    if (Array.isArray(value)) {
      const encoded = value.map(v => ZbxCrypto.rlpEncode(v));
      const total   = encoded.reduce((s, b) => s + b.length, 0);
      const prefix  = rlpListPrefix(total);
      const result  = new Uint8Array(prefix.length + total);
      let offset = 0;
      result.set(prefix, offset); offset += prefix.length;
      for (const b of encoded) { result.set(b, offset); offset += b.length; }
      return result;
    }
    if (typeof value === "string") {
      return ZbxCrypto.rlpEncode(hexToBytes(value.startsWith("0x") ? value.slice(2) : value));
    }
    if (value.length === 0) return new Uint8Array([0x80]);
    if (value.length === 1 && value[0] < 0x80) return value;
    const prefix = rlpStringPrefix(value.length);
    const result = new Uint8Array(prefix.length + value.length);
    result.set(prefix); result.set(value, prefix.length);
    return result;
  },
};

function hexToBytes(hex: string): Uint8Array {
  const clean = hex.replace(/^0x/, "");
  const bytes = new Uint8Array(clean.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  }
  return bytes;
}

function rlpStringPrefix(len: number): Uint8Array {
  if (len < 56) return new Uint8Array([0x80 + len]);
  const lenBytes = numberToBytes(len);
  return new Uint8Array([0xb7 + lenBytes.length, ...lenBytes]);
}

function rlpListPrefix(len: number): Uint8Array {
  if (len < 56) return new Uint8Array([0xc0 + len]);
  const lenBytes = numberToBytes(len);
  return new Uint8Array([0xf7 + lenBytes.length, ...lenBytes]);
}

function numberToBytes(n: number): Uint8Array {
  const bytes: number[] = [];
  while (n > 0) { bytes.unshift(n & 0xff); n >>= 8; }
  return new Uint8Array(bytes);
}
