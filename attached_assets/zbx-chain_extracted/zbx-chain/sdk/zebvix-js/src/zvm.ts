/**
 * ZvmHelper — ZVM (Zebvix VM) utilities.
 * Accessible via `client.zvm.*`
 */
import type { ZbxClient } from "./client";

export class ZvmHelper {
  /** ZVM version (matches CHAINVER opcode 0xC5) */
  readonly version = 1;

  /** ZBX block time in ms (matches ZBXTIME opcode 0xC3) */
  readonly blockTimeMs = 5000;

  /** ZVM magic prefix bytes for native contracts */
  readonly magic = "ef5a42";

  constructor(private client: ZbxClient) {}

  /**
   * Get current ZBX/USD price (same as ZBXPRICE opcode 0xC2).
   *
   * @example
   * const price = await zbx.zvm.price();
   * console.log(`1 ZBX = \${price} USD`);
   */
  async price(): Promise<string> {
    const info = await this.client.rpc<{ zbxUsd: string }>("zbx_getPriceUSD");
    return info.zbxUsd;
  }

  /**
   * Check if a contract is ZVM-native (has 0xEF5A42 magic prefix).
   *
   * @example
   * const code = await zbx.rpc("eth_getCode", ["0x742d35...", "latest"]);
   * zbx.zvm.isNative(code); // true if ZVM-native contract
   */
  isNative(bytecodeHex: string): boolean {
    const hex = bytecodeHex.startsWith("0x") ? bytecodeHex.slice(2) : bytecodeHex;
    return hex.toLowerCase().startsWith(this.magic);
  }

  /**
   * Decode a ZBXPRICE opcode return value (uint256, 18 decimals) to USD string.
   *
   * @example
   * const raw = "0x0000000000000000000000000000000000000000000878678326eac9000000";
   * zbx.zvm.decodePriceHex(raw); // "2500"
   */
  decodePriceHex(hex: string): string {
    const wei = BigInt(hex);
    return (wei / 10n ** 18n).toString();
  }

  /**
   * Disassemble ZVM bytecode into readable opcodes.
   * Returns array of opcode strings.
   *
   * @example
   * zbx.zvm.disassemble("6003600401c200");
   * // ["0000 60 PUSH1 03", "0002 60 PUSH1 04", "0004 01 ADD", "0005 c2 ZBXPRICE", "0006 00 STOP"]
   */
  disassemble(bytecodeHex: string): string[] {
    const hex = bytecodeHex.startsWith("0x") ? bytecodeHex.slice(2) : bytecodeHex;
    const bytes = hex.match(/.{2}/g)?.map(b => parseInt(b, 16)) ?? [];
    const lines: string[] = [];
    let i = 0;

    const name = (b: number) => ZVM_OPCODE_NAMES[b] ?? "UNKNOWN";

    while (i < bytes.length) {
      const b = bytes[i];
      const pc = i.toString(16).padStart(4, "0");

      if (b >= 0x60 && b <= 0x7f) {
        const n = b - 0x5f;
        const imm = bytes.slice(i + 1, i + 1 + n).map(x => x.toString(16).padStart(2, "0")).join("");
        lines.push(`\${pc}  \${b.toString(16).padStart(2, "0")}  PUSH\${n}  0x\${imm}`);
        i += 1 + n;
      } else if (b >= 0x80 && b <= 0x8f) {
        lines.push(`\${pc}  \${b.toString(16).padStart(2, "0")}  DUP\${b - 0x7f}`);
        i++;
      } else if (b >= 0x90 && b <= 0x9f) {
        lines.push(`\${pc}  \${b.toString(16).padStart(2, "0")}  SWAP\${b - 0x8f}`);
        i++;
      } else {
        const isZvm = b >= 0xc0 && b <= 0xc9;
        const suffix = isZvm ? "  ← ZVM" : "";
        lines.push(`\${pc}  \${b.toString(16).padStart(2, "0")}  \${name(b)}\${suffix}`);
        i++;
      }
    }
    return lines;
  }
}

const ZVM_OPCODE_NAMES: Record<number, string> = {
  0x00: "STOP",     0x01: "ADD",      0x02: "MUL",      0x03: "SUB",
  0x04: "DIV",      0x05: "SDIV",     0x06: "MOD",      0x07: "SMOD",
  0x08: "ADDMOD",   0x09: "MULMOD",   0x0a: "EXP",      0x0b: "SIGNEXTEND",
  0x10: "LT",       0x11: "GT",       0x12: "SLT",      0x13: "SGT",
  0x14: "EQ",       0x15: "ISZERO",   0x16: "AND",      0x17: "OR",
  0x18: "XOR",      0x19: "NOT",      0x1a: "BYTE",     0x1b: "SHL",
  0x1c: "SHR",      0x1d: "SAR",      0x20: "KECCAK256",
  0x30: "ADDRESS",  0x31: "BALANCE",  0x32: "ORIGIN",   0x33: "CALLER",
  0x34: "CALLVALUE",0x35: "CALLDATALOAD",0x40: "BLOCKHASH",
  0x41: "COINBASE", 0x42: "TIMESTAMP",0x43: "NUMBER",   0x44: "PREVRANDAO",
  0x45: "GASLIMIT", 0x46: "CHAINID",  0x47: "SELFBALANCE",0x48: "BASEFEE",
  0x49: "BLOBHASH", 0x4a: "BLOBBASEFEE",
  0x50: "POP",      0x51: "MLOAD",    0x52: "MSTORE",   0x53: "MSTORE8",
  0x54: "SLOAD",    0x55: "SSTORE",   0x56: "JUMP",     0x57: "JUMPI",
  0x58: "PC",       0x59: "MSIZE",    0x5a: "GAS",      0x5b: "JUMPDEST",
  0x5c: "TLOAD",    0x5d: "TSTORE",   0x5e: "MCOPY",    0x5f: "PUSH0",
  0xa0: "LOG0",     0xa1: "LOG1",     0xa2: "LOG2",     0xa3: "LOG3", 0xa4: "LOG4",
  0xf0: "CREATE",   0xf1: "CALL",     0xf2: "CALLCODE", 0xf3: "RETURN",
  0xf4: "DELEGATECALL",0xf5: "CREATE2",0xfa: "STATICCALL",
  0xfd: "REVERT",   0xfe: "INVALID",  0xff: "SELFDESTRUCT",
  // ZVM native opcodes
  0xc0: "PAYID",    0xc1: "ZUSDBAL",  0xc2: "ZBXPRICE", 0xc3: "ZBXTIME",
  0xc4: "AASENDER", 0xc5: "CHAINVER", 0xc6: "BLOBFEE",  0xc7: "PAYIDSET",
  0xc8: "ZBXBURN",  0xc9: "ZVMLOG",
};