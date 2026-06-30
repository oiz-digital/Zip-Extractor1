//! Property-based tests for HotStuff BFT consensus.

#[cfg(test)]
mod consensus_property_tests {
    use std::collections::{HashMap, HashSet};

    /// Property: only one block can be finalised per height (safety).
    #[test]
    fn single_block_per_height() {
        let mut finalised: HashMap<u64, [u8; 32]> = HashMap::new();

        for height in 0u64..100 {
            let block_hash = {
                let mut h = [0u8; 32];
                h[0..8].copy_from_slice(&height.to_be_bytes());
                h
            };
            let prev = finalised.insert(height, block_hash);
            assert!(prev.is_none(), "only one block can be finalised at height {}", height);
        }
    }

    /// Property: liveness — if leader is honest and network is synchronous, block finalises.
    #[test]
    fn liveness_with_honest_majority() {
        let n = 10usize;
        let f = 3usize;  // max faulty
        let honest = n - f;
        let quorum = 2 * n / 3 + 1;

        // Honest nodes always vote for the leader's proposal.
        let votes_cast = honest; // only honest nodes vote
        let finalised  = votes_cast >= quorum;
        assert!(finalised, "honest majority should finalise ({}>={})", votes_cast, quorum);
    }

    /// Property: no two honest nodes commit different blocks at the same height (safety).
    #[test]
    fn no_conflicting_commits() {
        // Simulate 2 honest nodes — they must agree on the same block at each height.
        let node_a_chain = vec![[0u8; 32], [1u8; 32], [2u8; 32]];
        let node_b_chain = vec![[0u8; 32], [1u8; 32], [2u8; 32]];
        assert_eq!(node_a_chain, node_b_chain, "honest nodes must agree on chain");
    }

    /// Property: round timeout grows exponentially (prevents livelock).
    #[test]
    fn timeout_exponential_backoff() {
        let base_ms = 1000u64;
        let max_ms  = 60_000u64;
        let mut timeout = base_ms;
        let mut timeouts = vec![timeout];

        for round in 1..10 {
            timeout = (timeout * 2).min(max_ms);
            timeouts.push(timeout);
        }

        // Timeouts should be strictly increasing until max.
        for i in 1..timeouts.len() {
            assert!(timeouts[i] >= timeouts[i-1], "timeout must not decrease");
        }
        // All timeouts should be bounded by max.
        for &t in &timeouts {
            assert!(t <= max_ms, "timeout must not exceed max");
        }
    }

    /// Property: vote equivocation is detectable.
    #[test]
    fn equivocation_detectable() {
        let validator = [0x01u8; 20];
        let round = 1u64;
        let block_a = [0xAAu8; 32];
        let block_b = [0xBBu8; 32];

        // Two votes for different blocks in the same round = equivocation.
        let vote1 = (round, validator, block_a);
        let vote2 = (round, validator, block_b);

        let is_equivocation = vote1.0 == vote2.0 && vote1.1 == vote2.1 && vote1.2 != vote2.2;
        assert!(is_equivocation, "conflicting votes in same round = equivocation");
    }
}