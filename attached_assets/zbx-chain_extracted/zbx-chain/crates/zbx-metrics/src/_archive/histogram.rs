//! Histogram metrics — latency distributions for block/tx processing.

/// A simple histogram for measuring operation durations.
/// Tracks counts per bucket and computes percentiles on demand.
#[derive(Debug, Clone)]
pub struct Histogram {
    pub name:    String,
    buckets:     Vec<(f64, u64)>,  // (upper_bound, count)
    pub sum:     f64,
    pub count:   u64,
}

impl Histogram {
    /// Create a histogram with standard latency buckets (milliseconds).
    pub fn new_latency(name: &str) -> Self {
        Self {
            name: name.into(),
            buckets: vec![
                (0.1, 0), (0.5, 0), (1.0, 0), (5.0, 0),
                (10.0, 0), (50.0, 0), (100.0, 0), (500.0, 0),
                (1000.0, 0), (5000.0, 0), (f64::INFINITY, 0),
            ],
            sum: 0.0,
            count: 0,
        }
    }

    /// Record a single observation (value in ms).
    pub fn observe(&mut self, value: f64) {
        self.sum   += value;
        self.count += 1;
        for (upper, count) in self.buckets.iter_mut() {
            if value <= *upper { *count += 1; }
        }
    }

    /// Compute an approximate percentile (linear interpolation between buckets).
    pub fn percentile(&self, p: f64) -> f64 {
        if self.count == 0 { return 0.0; }
        let target = (self.count as f64 * p / 100.0) as u64;
        let mut prev_upper = 0.0;
        let mut prev_count = 0u64;
        for &(upper, count) in &self.buckets {
            if count >= target {
                if count == prev_count { return prev_upper; }
                let frac = (target - prev_count) as f64 / (count - prev_count) as f64;
                return prev_upper + (upper - prev_upper) * frac;
            }
            prev_upper = upper;
            prev_count = count;
        }
        self.buckets.last().map(|(u, _)| *u).unwrap_or(0.0)
    }

    pub fn mean(&self) -> f64 {
        if self.count == 0 { return 0.0; }
        self.sum / self.count as f64
    }
}

/// Pre-built histograms for common ZBX Chain operations.
pub struct ZbxHistograms {
    pub block_import_ms:       Histogram,
    pub tx_validation_ms:      Histogram,
    pub evm_execution_ms:      Histogram,
    pub rpc_request_ms:        Histogram,
    pub p2p_message_ms:        Histogram,
    pub proof_generation_ms:   Histogram,
    pub state_trie_lookup_ms:  Histogram,
}

impl ZbxHistograms {
    pub fn new() -> Self {
        Self {
            block_import_ms:      Histogram::new_latency("zbx_block_import_ms"),
            tx_validation_ms:     Histogram::new_latency("zbx_tx_validation_ms"),
            evm_execution_ms:     Histogram::new_latency("zbx_evm_execution_ms"),
            rpc_request_ms:       Histogram::new_latency("zbx_rpc_request_ms"),
            p2p_message_ms:       Histogram::new_latency("zbx_p2p_message_ms"),
            proof_generation_ms:  Histogram::new_latency("zbx_proof_generation_ms"),
            state_trie_lookup_ms: Histogram::new_latency("zbx_state_trie_lookup_ms"),
        }
    }
}

impl Default for ZbxHistograms { fn default() -> Self { Self::new() } }