/**
 * StakingHelper — ZbxStaking contract interaction.
 * Accessible via `client.staking.*`
 *
 * Covers: stake, unstake, claim rewards, view pending rewards and stake info.
 * Contract: ZbxStaking.sol — ZEP-018
 *
 * @example
 * import { ZbxClient } from "zebvix.js";
 *
 * const zbx   = new ZbxClient("https://rpc.zebvix.io");
 * const wallet = zbx.wallet(process.env.PRIVATE_KEY!);
 *
 * // Stake 1000 ZBX
 * const hash = await wallet.staking.stake("1000");
 * console.log("Staked:", hash);
 *
 * // Check pending rewards
 * const pending = await zbx.staking.pendingReward("0xYourAddress");
 * console.log("Pending ZBX rewards:", zbx.formatZbx(pending));
 *
 * // Claim all rewards
 * const claimHash = await wallet.staking.claim();
 * console.log("Claimed:", claimHash);
 */
import type { ZbxClient } from "./client";

// ─── Types ───────────────────────────────────────────────────────────────────

export interface StakeInfo {
  /** Amount currently staked (wei). */
  stakedWei:    bigint;
  /** Amount currently staked (formatted ZBX string). */
  staked:       string;
  /** Accumulated unclaimed rewards (wei). */
  pendingWei:   bigint;
  /** Accumulated unclaimed rewards (formatted ZBX string). */
  pending:      string;
  /** Timestamp when the stake was last updated. */
  lastUpdate:   number;
  /** Whether MIN_STAKE_AGE has passed and rewards can be claimed. */
  claimable:    boolean;
}

export interface StakingStats {
  /** Total ZBX staked across all participants (wei). */
  totalStakedWei:  bigint;
  /** Total ZBX staked (formatted string). */
  totalStaked:     string;
  /** Current reward emission rate (ZBX per second, wei). */
  rewardRateWei:   bigint;
  /** Reward rate (formatted ZBX/s string). */
  rewardRate:      string;
  /** Estimated APY based on current rate and total staked (percentage string). */
  estimatedApy:    string;
}

// ─── ABI selectors (keccak256(sig)[0:4]) ─────────────────────────────────────

const SEL = {
  stake:         "0xa694fc3a", // stake(uint256)
  unstake:       "0x2e17de78", // unstake(uint256)
  claim:         "0x4e71d92d", // claim()
  pendingReward: "0xf40f0f52", // pendingReward(address)
  users:         "0x1959a002", // users(address) → (uint256 stake, uint256 pending, uint256 rewardDebt, uint256 lastStakeTime)
  totalStaked:   "0x817b1cd2", // totalStaked()
  rewardRate:    "0x7b0a47ee", // rewardRate()
  accRewardPerShare: "0x98e5b1e3", // accRewardPerShare()
} as const;

// ─── Helper ──────────────────────────────────────────────────────────────────

export class StakingHelper {
  constructor(
    private readonly client: ZbxClient,
    /** Deployed contract address — defaults to the canonical ZEP-018 mainnet address. */
    readonly contractAddress: string = "0x000000000000000000000000005a425354414b45",
  ) {}

  // ── Views ──────────────────────────────────────────────────────────────────

  /**
   * Get stake info for an address.
   *
   * @example
   * const info = await zbx.staking.getStakeInfo("0xYourAddress");
   * console.log("Staked:", info.staked, "ZBX");
   * console.log("Pending:", info.pending, "ZBX");
   */
  async getStakeInfo(address: string): Promise<StakeInfo> {
    const data = SEL.users + address.slice(2).padStart(64, "0");
    const raw  = await this._call(data);
    const buf  = raw.replace(/^0x/, "");

    const stakedWei  = hexToBigInt(buf.slice(0,   64));
    const pendingWei = hexToBigInt(buf.slice(64,  128));
    // rewardDebt at 128..192 (internal accounting — not exposed)
    const lastStakeTime = Number(hexToBigInt(buf.slice(192, 256)));

    const now          = Math.floor(Date.now() / 1000);
    const MIN_STAKE_AGE = 3600; // 1 hour (matches ZbxStaking.MIN_STAKE_AGE)
    const claimable    = (now - lastStakeTime) >= MIN_STAKE_AGE;

    // Pending reward from view function (includes live accrual)
    const pendingViewRaw = await this._call(SEL.pendingReward + address.slice(2).padStart(64, "0"));
    const pendingLive    = hexToBigInt(pendingViewRaw.replace(/^0x/, "").slice(0, 64));

    return {
      stakedWei,
      staked:    fmtWei(stakedWei),
      pendingWei: pendingLive,
      pending:    fmtWei(pendingLive),
      lastUpdate: lastStakeTime,
      claimable,
    };
  }

  /**
   * Get global staking statistics.
   *
   * @example
   * const stats = await zbx.staking.getStats();
   * console.log("Total staked:", stats.totalStaked, "ZBX");
   * console.log("Estimated APY:", stats.estimatedApy, "%");
   */
  async getStats(): Promise<StakingStats> {
    const [tsRaw, rrRaw] = await Promise.all([
      this._call(SEL.totalStaked),
      this._call(SEL.rewardRate),
    ]);

    const totalStakedWei = hexToBigInt(tsRaw.replace(/^0x/, "").slice(0, 64));
    const rewardRateWei  = hexToBigInt(rrRaw.replace(/^0x/, "").slice(0, 64));

    // APY = (rewardRate × 365 × 86400 / totalStaked) × 100
    let apy = "0.00";
    if (totalStakedWei > 0n) {
      const yearlyReward = rewardRateWei * 31_536_000n; // per year in wei
      const apyBps = (yearlyReward * 10_000n) / totalStakedWei;
      apy = (Number(apyBps) / 100).toFixed(2);
    }

    return {
      totalStakedWei,
      totalStaked:  fmtWei(totalStakedWei),
      rewardRateWei,
      rewardRate:   fmtWei(rewardRateWei) + "/s",
      estimatedApy: apy,
    };
  }

  /**
   * Get pending rewards for an address (live, includes current block accrual).
   *
   * @example
   * const pending = await zbx.staking.pendingReward("0xYourAddress");
   * console.log("Claimable:", zbx.formatZbx(pending), "ZBX");
   */
  async pendingReward(address: string): Promise<bigint> {
    const data = SEL.pendingReward + address.slice(2).padStart(64, "0");
    const raw  = await this._call(data);
    return hexToBigInt(raw.replace(/^0x/, "").slice(0, 64));
  }

  // ── Transactions (return encoded calldata; wallet submits) ─────────────────

  /**
   * Encode `stake(amount)` calldata.
   * The wallet must send ZBX value = amount when calling this.
   * Use `wallet.staking.stake(amount)` for a one-call helper.
   *
   * @example
   * const data = zbx.staking.encodeStake("1000");
   * // submit via wallet.sendTx({ to: zbx.staking.contractAddress, data, value: parseZbx("1000") })
   */
  encodeStake(amountZbx: string): string {
    return SEL.stake + pWei(amountZbx).toString(16).padStart(64, "0");
  }

  /**
   * Encode `unstake(amount)` calldata.
   *
   * @example
   * const data = zbx.staking.encodeUnstake("500");
   */
  encodeUnstake(amountZbx: string): string {
    return SEL.unstake + pWei(amountZbx).toString(16).padStart(64, "0");
  }

  /**
   * Encode `claim()` calldata.
   *
   * @example
   * const data = zbx.staking.encodeClaim();
   */
  encodeClaim(): string {
    return SEL.claim;
  }

  // ── Private ────────────────────────────────────────────────────────────────

  private async _call(data: string): Promise<string> {
    return this.client.rpc<string>("eth_call", [
      { to: this.contractAddress, data },
      "latest",
    ]);
  }
}

// ── Utilities ─────────────────────────────────────────────────────────────────

function hexToBigInt(hex: string): bigint {
  return hex ? BigInt("0x" + hex) : 0n;
}

function fmtWei(wei: bigint, dec = 4): string {
  const whole  = wei / 10n ** 18n;
  const frac   = wei % 10n ** 18n;
  const fracStr = frac.toString().padStart(18, "0").slice(0, dec).replace(/0+$/, "");
  return fracStr ? `${whole}.${fracStr}` : whole.toString();
}

function pWei(amount: string): bigint {
  const [w, f = ""] = amount.split(".");
  return BigInt(w) * 10n ** 18n + BigInt(f.padEnd(18, "0").slice(0, 18) || "0");
}
