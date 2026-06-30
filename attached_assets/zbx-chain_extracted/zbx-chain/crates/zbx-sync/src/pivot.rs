//! Pivot block selection for snap-sync.

use crate::error::SyncError;
use crate::fast_sync::BlockNumber;
use tracing::info;

const SAFE_PIVOT_CONFIRMATIONS: u64 = 64;

pub struct PivotSelector {
    chain_tip: BlockNumber,
    finalized: BlockNumber,
}

impl PivotSelector {
    pub fn new(chain_tip: BlockNumber, finalized: BlockNumber) -> Self {
        Self { chain_tip, finalized }
    }

    pub fn pivot_height(&self) -> Result<BlockNumber, SyncError> {
        if self.finalized < SAFE_PIVOT_CONFIRMATIONS {
            return Err(SyncError::PivotNotFinalized(0));
        }
        let pivot = self.finalized.saturating_sub(SAFE_PIVOT_CONFIRMATIONS);
        info!("snap-sync: selected pivot block {}", pivot);
        Ok(pivot)
    }

    /// Validate that `height` is at or below our finalized checkpoint.
    pub fn validate_pivot_height(height: BlockNumber, finalized: BlockNumber) -> Result<(), SyncError> {
        if height > finalized {
            Err(SyncError::PivotNotFinalized(height))
        } else {
            Ok(())
        }
    }
}
