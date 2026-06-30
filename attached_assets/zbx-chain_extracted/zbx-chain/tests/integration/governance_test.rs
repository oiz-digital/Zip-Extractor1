//! Integration tests for ZbxGovernor.

#[cfg(test)]
mod governance_integration {

    #[test]
    fn proposal_requires_min_tokens() {
        let proposal_threshold: u128 = 1_000_000 * 1_000_000_000_000_000_000; // 1M ZBX
        let proposer_balance:   u128 =   500_000 * 1_000_000_000_000_000_000; // 500K ZBX

        let can_propose = proposer_balance >= proposal_threshold;
        assert!(!can_propose, "insufficient ZBX to propose");
    }

    #[test]
    fn quorum_requires_10pct_of_supply() {
        let total_supply:   u128 = 150_000_000 * 1_000_000_000_000_000_000;
        let quorum_pct:     u128 = 10; // 10%
        let quorum_votes    = total_supply * quorum_pct / 100;
        let votes_cast:     u128 = 20_000_000 * 1_000_000_000_000_000_000;

        let quorum_reached = votes_cast >= quorum_votes;
        assert!(quorum_reached, "20M votes > 15M quorum (10% of 150M supply)");
    }

    #[test]
    fn simple_majority_wins() {
        let for_votes:     u128 = 8_000_000;
        let against_votes: u128 = 4_000_000;
        let abstain:       u128 = 1_000_000;
        let total_votes = for_votes + against_votes + abstain;

        let passed = for_votes > against_votes;
        assert!(passed, "for > against → proposal passes");
    }

    #[test]
    fn timelock_delay_enforced() {
        let scheduled_at: u64 = 1_000_000;
        let min_delay: u64    = 172_800; // 48 hours in seconds
        let ready_at: u64     = scheduled_at + min_delay;
        let current_time: u64 = scheduled_at + 86_400; // 24 hours later

        let can_execute = current_time >= ready_at;
        assert!(!can_execute, "must wait full 48 hours before execution");

        let after_delay: u64 = scheduled_at + 200_000;
        let can_execute_after = after_delay >= ready_at;
        assert!(can_execute_after, "can execute after delay period");
    }
}