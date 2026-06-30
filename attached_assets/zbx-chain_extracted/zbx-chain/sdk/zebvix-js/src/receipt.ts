/**
 * Transaction receipt types and helpers.
 */

export interface TxReceipt {
  txHash:      string;
  blockHeight: number;
  blockHash:   string;
  from:        string;
  to:          string;
  status:      "success" | "failed" | "reverted";
  gasUsed:     bigint;
  fee:         bigint;
  logs:        TxLog[];
  kind:        string;
}

export interface TxLog {
  address:  string;
  topics:   string[];
  data:     string;
  logIndex: number;
}

/** Check if a receipt is a ZVM-native execution */
export function isZvmReceipt(receipt: TxReceipt): boolean {
  return receipt.logs.some(l =>
    l.topics[0] === "0x5a564d4c4f470000000000000000000000000000000000000000000000000000"
  );
}

/** Extract ZVMLOG entries from receipt logs */
export function extractZvmLogs(receipt: TxReceipt): Array<{ key: string; value: string }> {
  return receipt.logs
    .filter(l => l.topics[0]?.startsWith("0x5a564d4c"))
    .map(l => {
      try {
        const raw  = Buffer.from(l.data.replace("0x", ""), "hex").toString("utf8");
        const [key, value] = raw.split("=");
        return { key: key?.trim() ?? "", value: value?.trim() ?? "" };
      } catch {
        return { key: "", value: l.data };
      }
    });
}

/** Format fee in ZBX */
export function formatFee(receipt: TxReceipt): string {
  const whole = receipt.fee / 10n ** 18n;
  const frac  = receipt.fee % 10n ** 18n;
  const fracStr = frac.toString().padStart(18, "0").slice(0, 6).replace(/0+$/, "");
  return fracStr ? `\${whole}.\${fracStr} ZBX` : `\${whole} ZBX`;
}