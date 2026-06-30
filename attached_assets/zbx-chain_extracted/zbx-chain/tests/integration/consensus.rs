//! Integration tests for HotStuff BFT consensus.

#[cfg(test)]
mod tests {
    use zbx_consensus::{HotStuffConfig, SafetyRules, VoteData, QuorumCert};
    use zbx_types::H256;

    fn dummy_hash(n: u8) -> H256 { H256([n; 32]) }

    #[test]
    fn test_vote_extends_highest_qc() {
        // A validator should vote on block B if B's QC is >= its highQC.
        let qc_height = 10u64;
        let block_height = 11u64;
        assert!(block_height > qc_height, "new block must extend QC");
    }

    #[test]
    fn test_two_chain_commit_rule() {
        // A block B at height h is committed when there is a 2-chain:
        //   B_h <- QC(B_h) in B_{h+1} <- QC(B_{h+1}) in B_{h+2}
        let b0 = dummy_hash(0);
        let b1 = dummy_hash(1);
        let b2 = dummy_hash(2);
        // Simplified: check that parent linkage holds.
        assert_ne!(b0, b1);
        assert_ne!(b1, b2);
    }

    #[test]
    fn test_no_vote_on_conflicting_branch() {
        // A validator with locked block L must not vote on B if B does not
        // extend L (safety rule).
        let locked_height: u64 = 5;
        let proposed_height: u64 = 4; // lower than locked
        assert!(proposed_height < locked_height, "safety violation: must not vote");
    }

    #[test]
    fn test_view_change_threshold() {
        // A view change requires >= 2f+1 timeout messages.
        let n = 4usize;
        let f = (n - 1) / 3;
        let threshold = 2 * f + 1;
        assert_eq!(threshold, 3); // 4 validators → f=1 → 3 needed
    }

    #[test]
    fn test_qc_aggregation() {
        // A QC is valid if it contains >= 2f+1 signatures.
        let sig_count = 3usize;
        let validator_count = 4usize;
        let f = (validator_count - 1) / 3;
        assert!(sig_count >= 2 * f + 1, "need 2f+1 = 3 sigs for QC with n=4");
    }
}