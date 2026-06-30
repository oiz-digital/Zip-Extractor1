//! STARK / FRI protocol parameters.
//!
//! The parameter set determines security level, proof size, and proving time.
//! ZBX Chain uses separate parameter sets for different proof types:
//!   - Fast: smaller proofs, weaker security (for state proofs, light clients)
//!   - Standard: 128-bit security (for block proofs, bridge fraud proofs)
//!   - Recursive: optimised for proof aggregation

use serde::{Deserialize, Serialize};

/// FRI protocol parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FriParams {
    /// Blowup factor (rate = 1/blowup). Must be a power of 2.
    /// Higher = more secure, bigger proof. Typical: 4 or 8.
    pub blowup_factor:   usize,
    /// Number of FRI queries per round.
    /// More queries = higher security. Typical: 40 for 128-bit security.
    pub num_queries:     usize,
    /// Number of reduction layers in FRI folding phase.
    pub reduction_arity: usize,
    /// Degree of the final FRI polynomial (must fit in memory).
    pub max_remainder_degree: usize,
}

/// STARK trace parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceParams {
    /// Trace width: number of columns (registers/variables per row).
    pub num_columns: usize,
    /// Trace height: number of rows (execution steps). Must be power of 2.
    pub trace_height: usize,
    /// Number of auxiliary columns (permutation arguments, lookup tables).
    pub num_aux_columns: usize,
}

/// Full prover parameter set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProverParams {
    pub name:   String,
    pub fri:    FriParams,
    pub trace:  TraceParams,
    /// Security level in bits.
    pub security_bits: usize,
    /// Approximate proof size in bytes.
    pub proof_size_estimate: usize,
}

impl ProverParams {
    /// Fast parameters: ~64-bit security, small proofs (~800 bytes).
    /// Use for state proofs and light client queries.
    pub fn fast() -> Self {
        Self {
            name: "fast".into(),
            security_bits: 64,
            proof_size_estimate: 800,
            fri: FriParams {
                blowup_factor:    2,
                num_queries:      20,
                reduction_arity:  2,
                max_remainder_degree: 4,
            },
            trace: TraceParams {
                num_columns:      64,
                trace_height:     1024,
                num_aux_columns:  8,
            },
        }
    }

    /// Standard parameters: 128-bit security, medium proofs (~320 KB).
    /// Use for block execution proofs and fraud proofs.
    pub fn standard() -> Self {
        Self {
            name: "standard".into(),
            security_bits: 128,
            proof_size_estimate: 327_680,
            fri: FriParams {
                blowup_factor:    4,
                num_queries:      40,
                reduction_arity:  2,
                max_remainder_degree: 4,
            },
            trace: TraceParams {
                num_columns:      256,
                trace_height:     1 << 20, // 1M rows
                num_aux_columns:  32,
            },
        }
    }

    /// Recursive parameters: optimised for proof aggregation.
    /// Inner proofs use BN254 for efficient recursion.
    pub fn recursive() -> Self {
        Self {
            name: "recursive".into(),
            security_bits: 128,
            proof_size_estimate: 49_152, // ~48 KB compressed
            fri: FriParams {
                blowup_factor:    4,
                num_queries:      40,
                reduction_arity:  4, // higher arity = fewer layers = faster recursion
                max_remainder_degree: 16,
            },
            trace: TraceParams {
                num_columns:      128,
                trace_height:     1 << 18, // 256K rows
                num_aux_columns:  16,
            },
        }
    }

    pub fn is_valid(&self) -> bool {
        self.fri.blowup_factor.is_power_of_two()
            && self.trace.trace_height.is_power_of_two()
            && self.fri.num_queries >= 20
            && self.trace.num_columns > 0
    }
}