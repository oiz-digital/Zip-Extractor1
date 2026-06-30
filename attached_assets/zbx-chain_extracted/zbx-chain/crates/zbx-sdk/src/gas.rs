//! Gas price oracle: EIP-1559 fee suggestion with percentile analysis.

use crate::error::SdkError;
use zbx_types::U256;
use serde_json::Value;

/// Suggested EIP-1559 fee parameters.
#[derive(Debug, Clone)]
pub struct GasPricing {
    /// Suggested max fee per gas (base_fee + max_priority).
    pub max_fee:          U256,
    /// Suggested max priority fee (tip).
    pub max_priority_fee: U256,
    /// Estimated base fee of the next block.
    pub next_base_fee:    U256,
}

impl GasPricing {
    /// Parse a `GasPricing` from an `eth_feeHistory` response.
    pub fn from_fee_history(history: &Value) -> Result<Self, SdkError> {
        // base_fees = history["baseFeePerGas"] array (includes next block estimate)
        let base_fees = history["baseFeePerGas"].as_array()
            .ok_or_else(|| SdkError::RpcParse("missing baseFeePerGas".into()))?;
        let next_base_fee = base_fees.last()
            .and_then(|v| v.as_str())
            .map(|s| parse_hex_u256(s))
            .transpose()?
            .unwrap_or(U256::from(1_000_000_000u64)); // 1 gwei fallback

        // reward = history["reward"] array of arrays
        // Use the 50th percentile (median) tip.
        let tips: Vec<U256> = history["reward"].as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|block_rewards| block_rewards.as_array()?.get(1)?.as_str())
            .filter_map(|s| parse_hex_u256(s).ok())
            .collect();

        let median_tip = if tips.is_empty() {
            U256::from(1_500_000_000u64) // 1.5 gwei default
        } else {
            let mut sorted = tips.clone();
            sorted.sort();
            sorted[sorted.len() / 2]
        };

        // max_fee = 2 * base_fee + tip (leaves headroom for base_fee increases)
        let max_fee = next_base_fee * U256::from(2) + median_tip;

        Ok(Self {
            max_fee,
            max_priority_fee: median_tip,
            next_base_fee,
        })
    }

    pub fn print(&self) {
        println!(
            "Gas: base={:.2} gwei, priority={:.2} gwei, max={:.2} gwei",
            u256_to_gwei(self.next_base_fee),
            u256_to_gwei(self.max_priority_fee),
            u256_to_gwei(self.max_fee),
        );
    }
}

/// Gas oracle that queries chain data and caches results.
pub struct GasOracle {
    cached:      Option<GasPricing>,
    last_update: std::time::Instant,
    ttl:         std::time::Duration,
}

impl GasOracle {
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            cached:      None,
            last_update: std::time::Instant::now(),
            ttl:         std::time::Duration::from_secs(ttl_secs),
        }
    }

    pub async fn get(
        &mut self,
        provider: &crate::provider::Provider,
    ) -> Result<GasPricing, SdkError> {
        if self.cached.is_none() || self.last_update.elapsed() > self.ttl {
            let pricing = provider.get_gas_pricing().await?;
            self.cached      = Some(pricing);
            self.last_update = std::time::Instant::now();
        }
        Ok(self.cached.as_ref().unwrap().clone())
    }
}

fn parse_hex_u256(hex: &str) -> Result<U256, SdkError> {
    let n = u128::from_str_radix(hex.trim_start_matches("0x"), 16)
        .map_err(|e| SdkError::RpcParse(e.to_string()))?;
    Ok(U256::from(n))
}

fn u256_to_gwei(u: U256) -> f64 {
    u.as_u128() as f64 / 1e9
}