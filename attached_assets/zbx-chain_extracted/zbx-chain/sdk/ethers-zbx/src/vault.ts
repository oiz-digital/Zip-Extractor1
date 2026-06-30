/**
 * Vault — ZusdVault CDP operations for @zebvix/ethers.
 *
 * @example
 * import { Vault, ZbxProvider, ZbxWallet, ethers } from "@zebvix/ethers";
 *
 * const provider = new ZbxProvider();
 * const wallet   = new ZbxWallet(privateKey, provider);
 * const vault    = new Vault(provider);
 *
 * // Check your CDP
 * const cdp = await vault.getCDP("0xYourAddress");
 * console.log("Collateral:", ethers.formatEther(cdp.collateral), "ZBX");
 * console.log("Debt:", ethers.formatEther(cdp.debt), "ZUSD");
 * console.log("CR:", cdp.crBps / 100, "%");
 *
 * // Open CDP: lock 1000 ZBX collateral
 * await wallet.sendTransaction({
 *   to:    vault.address,
 *   data:  vault.encodeOpenCDP(ethers.parseEther("1000")),
 *   value: ethers.parseEther("1000"),
 * });
 *
 * // Mint 500 ZUSD against it
 * await wallet.sendTransaction({
 *   to:   vault.address,
 *   data: vault.encodeMintMore(ethers.parseEther("500")),
 * });
 */
import { Contract, Interface, type Provider, type Signer } from "ethers";

const ABI = [
  // Views
  "function cdps(address owner) view returns (uint256 collateral, uint256 debt, uint256 lastFeeIndex)",
  "function collateralRatio(address owner) view returns (uint256 crBps, uint256 currentDebt)",
  "function maxMintableZusd(uint256 collateral) view returns (uint256)",
  "function totalCollateral() view returns (uint256)",
  "function totalDebt() view returns (uint256)",
  "function stabilityFeeBps() view returns (uint256)",
  // Writes
  "function openCDP(uint256 collateralAmount) payable",
  "function mintMore(uint256 zusdAmount)",
  "function repay(uint256 zusdAmount)",
  "function addCollateral(uint256 amount) payable",
  "function closeCDP()",
  "function liquidate(address cdpOwner)",
];

export interface CDPState {
  /** ZBX collateral (wei). */
  collateral:     bigint;
  /** ZUSD principal debt (wei). */
  debt:           bigint;
  /** Live debt with accrued stability fees (wei). */
  currentDebt:    bigint;
  /** Collateral ratio in basis points (e.g. 25000 = 250%). */
  crBps:          bigint;
  /** Whether this CDP exists. */
  exists:         boolean;
  /** Whether the CDP is currently liquidatable (crBps ≤ 10000). */
  liquidatable:   boolean;
}

export interface VaultStats {
  totalCollateral: bigint;
  totalDebt:       bigint;
  stabilityFeeBps: bigint;
}

export class Vault {
  readonly address: string;
  private readonly iface: Interface;
  private readonly contract: Contract;

  constructor(
    providerOrSigner: Provider | Signer,
    address = "0x000000000000000000000000005a425641554c54",
  ) {
    this.address  = address;
    this.iface    = new Interface(ABI);
    this.contract = new Contract(address, ABI, providerOrSigner);
  }

  /** Get CDP state for an owner address. */
  async getCDP(owner: string): Promise<CDPState> {
    const [cdp, cr] = await Promise.all([
      this.contract.cdps(owner),
      this.contract.collateralRatio(owner),
    ]);
    const collateral  = cdp.collateral as bigint;
    const debt        = cdp.debt as bigint;
    const currentDebt = cr.currentDebt as bigint;
    const crBps       = cr.crBps as bigint;
    return {
      collateral,
      debt,
      currentDebt,
      crBps,
      exists:       collateral > 0n,
      liquidatable: crBps > 0n && crBps <= 10_000n,
    };
  }

  /** Get the maximum ZUSD mintable for a given collateral amount (wei). */
  async maxMintableZusd(collateralWei: bigint): Promise<bigint> {
    return this.contract.maxMintableZusd(collateralWei) as Promise<bigint>;
  }

  /** Get global vault statistics. */
  async getStats(): Promise<VaultStats> {
    const [col, debt, fee] = await Promise.all([
      this.contract.totalCollateral(),
      this.contract.totalDebt(),
      this.contract.stabilityFeeBps(),
    ]);
    return {
      totalCollateral: col as bigint,
      totalDebt:       debt as bigint,
      stabilityFeeBps: fee as bigint,
    };
  }

  // ── Calldata encoders ────────────────────────────────────────────────────

  /** Encode `openCDP(uint256)` — send ZBX value alongside. */
  encodeOpenCDP(collateralWei: bigint): string {
    return this.iface.encodeFunctionData("openCDP", [collateralWei]);
  }

  /** Encode `mintMore(uint256)` — mint additional ZUSD. */
  encodeMintMore(zusdAmountWei: bigint): string {
    return this.iface.encodeFunctionData("mintMore", [zusdAmountWei]);
  }

  /** Encode `repay(uint256)` — repay ZUSD debt. */
  encodeRepay(zusdAmountWei: bigint): string {
    return this.iface.encodeFunctionData("repay", [zusdAmountWei]);
  }

  /** Encode `addCollateral(uint256)` — send ZBX value alongside. */
  encodeAddCollateral(collateralWei: bigint): string {
    return this.iface.encodeFunctionData("addCollateral", [collateralWei]);
  }

  /** Encode `closeCDP()` — repay all debt and withdraw all collateral. */
  encodeCloseCDP(): string {
    return this.iface.encodeFunctionData("closeCDP", []);
  }

  /** Encode `liquidate(address)` — liquidate an undercollateralised CDP. */
  encodeLiquidate(cdpOwner: string): string {
    return this.iface.encodeFunctionData("liquidate", [cdpOwner]);
  }
}
