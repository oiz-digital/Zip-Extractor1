/**
 * Staking — ZbxStaking contract wrapper for @zebvix/ethers.
 *
 * Uses ethers.js v6 Contract interface.
 *
 * @example
 * import { Staking, ZbxProvider } from "@zebvix/ethers";
 *
 * const provider  = new ZbxProvider();
 * const staking   = new Staking(provider);
 *
 * // Check pending rewards
 * const pending = await staking.pendingReward("0xYourAddress");
 * console.log("Pending:", pending, "ZBX (wei)");
 *
 * // Get full stake info
 * const info = await staking.getStakeInfo("0xYourAddress");
 * console.log("Staked:", info.staked, "Claimable:", info.claimable);
 *
 * // Build stake calldata (sign + send via ZbxWallet)
 * const tx = await wallet.sendTransaction({
 *   to:    staking.address,
 *   data:  staking.encodeStake(ethers.parseEther("1000")),
 *   value: ethers.parseEther("1000"),
 * });
 */
import { Contract, Interface, type Provider, type Signer } from "ethers";

const ABI = [
  "function stake(uint256 amount) payable",
  "function unstake(uint256 amount)",
  "function claim()",
  "function pendingReward(address user) view returns (uint256)",
  "function users(address) view returns (uint256 staked, uint256 pending, uint256 rewardDebt, uint256 lastStakeTime)",
  "function totalStaked() view returns (uint256)",
  "function rewardRate() view returns (uint256)",
];

const YEAR_SECS = 31_536_000n;
const MIN_STAKE_AGE = 3600; // 1 hour

export interface StakeInfo {
  stakedWei:   bigint;
  pendingWei:  bigint;
  lastUpdate:  number;
  claimable:   boolean;
}

export interface StakingStats {
  totalStakedWei: bigint;
  rewardRateWei:  bigint;
  /** Estimated APY as a fraction (e.g. 0.12 = 12%). */
  estimatedApy:   number;
}

export class Staking {
  readonly address: string;
  private readonly iface: Interface;
  private readonly contract: Contract;

  constructor(
    providerOrSigner: Provider | Signer,
    address = "0x000000000000000000000000005a425354414b45",
  ) {
    this.address  = address;
    this.iface    = new Interface(ABI);
    this.contract = new Contract(address, ABI, providerOrSigner);
  }

  /** Get live pending reward for a user (wei). */
  async pendingReward(user: string): Promise<bigint> {
    return this.contract.pendingReward(user) as Promise<bigint>;
  }

  /** Get full stake info for a user. */
  async getStakeInfo(user: string): Promise<StakeInfo> {
    const [tu, pending] = await Promise.all([
      this.contract.users(user),
      this.contract.pendingReward(user),
    ]);
    const lastUpdate  = Number(tu.lastStakeTime);
    const now         = Math.floor(Date.now() / 1000);
    return {
      stakedWei:  tu.staked as bigint,
      pendingWei: pending as bigint,
      lastUpdate,
      claimable:  (now - lastUpdate) >= MIN_STAKE_AGE,
    };
  }

  /** Get global staking stats. */
  async getStats(): Promise<StakingStats> {
    const [ts, rr] = await Promise.all([
      this.contract.totalStaked(),
      this.contract.rewardRate(),
    ]);
    const totalStakedWei = ts as bigint;
    const rewardRateWei  = rr as bigint;
    const apy = totalStakedWei > 0n
      ? Number((rewardRateWei * YEAR_SECS * 10_000n) / totalStakedWei) / 10_000
      : 0;
    return { totalStakedWei, rewardRateWei, estimatedApy: apy };
  }

  /** Encode `stake(uint256)` calldata. */
  encodeStake(amountWei: bigint): string {
    return this.iface.encodeFunctionData("stake", [amountWei]);
  }

  /** Encode `unstake(uint256)` calldata. */
  encodeUnstake(amountWei: bigint): string {
    return this.iface.encodeFunctionData("unstake", [amountWei]);
  }

  /** Encode `claim()` calldata. */
  encodeClaim(): string {
    return this.iface.encodeFunctionData("claim", []);
  }
}
