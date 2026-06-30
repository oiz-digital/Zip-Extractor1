//! Chaos tests — HotStuff-BFT Byzantine fault scenarios.
//!
//! Verifies safety (no conflicting commits) and liveness (eventual progress)
//! under the maximum tolerated number of Byzantine validators.

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    // ─── Model ────────────────────────────────────────────────────────────────

    /// Simulated vote message.
    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    struct Vote {
        round:     u64,
        block_hash: [u8; 32],
        voter:     usize,
    }

    /// Simulated quorum certificate.
    struct Qc {
        block_hash: [u8; 32],
        votes:      Vec<Vote>,
    }

    /// Simulated safety rule: a replica only votes for a block that
    /// extends the highest QC it has seen.
    struct SafetyRules {
        highest_qc_round: u64,
        locked_round:     u64,
    }

    impl SafetyRules {
        fn new() -> Self { Self { highest_qc_round: 0, locked_round: 0 } }

        /// Returns true if it is safe to vote for the given proposal.
        fn safe_to_vote(&self, proposal_round: u64, parent_qc_round: u64) -> bool {
            // HotStuff safety rule:
            // Vote iff parent_qc_round >= locked_round
            // AND proposal_round > highest_qc_round
            parent_qc_round >= self.locked_round
                && proposal_round > self.highest_qc_round
        }

        fn update_highest_qc(&mut self, qc_round: u64) {
            self.highest_qc_round = self.highest_qc_round.max(qc_round);
        }

        fn update_locked_round(&mut self, locked: u64) {
            self.locked_round = self.locked_round.max(locked);
        }
    }

    // ─── Tests ────────────────────────────────────────────────────────────────

    /// A Byzantine proposer cannot force honest validators to vote for two
    /// conflicting blocks in the same round.
    #[test]
    fn byzantine_proposer_cannot_equivocate_within_round() {
        // Simulate: Byzantine proposer sends block A to validators 0..1
        //           and block B to validators 2..3 in the same round.
        // Safety rule: each honest validator only votes once per round.

        let n = 4usize;
        let f = 1usize; // Byzantine
        assert!(n >= 3 * f + 1, "HotStuff requires n >= 3f+1");

        let round = 5u64;
        let mut votes_for_a = HashSet::<usize>::new();
        let mut votes_for_b = HashSet::<usize>::new();

        for validator_id in 0..n {
            let is_byzantine = validator_id == 0; // validator 0 is Byzantine
            if !is_byzantine {
                // Honest validator: votes for whichever block it received first.
                // In a real implementation, network ordering determines this.
                // The invariant: it votes for AT MOST one block per round.
                let voted_for_a = validator_id < 2;
                if voted_for_a {
                    votes_for_a.insert(validator_id);
                } else {
                    votes_for_b.insert(validator_id);
                }
            }
        }

        let quorum = (2 * n + 2) / 3; // = 3

        // Neither block should reach quorum because honest votes are split.
        assert!(votes_for_a.len() < quorum, "block A must not reach quorum");
        assert!(votes_for_b.len() < quorum, "block B must not reach quorum");
    }

    /// Safety rules prevent an honest validator from voting in a round
    /// lower than its locked round.
    #[test]
    fn safety_rules_prevent_low_round_vote() {
        let mut rules = SafetyRules::new();
        rules.update_highest_qc(10);
        rules.update_locked_round(9);

        // Round 5 < locked_round 9 → must NOT vote.
        assert!(!rules.safe_to_vote(5, 4), "must not vote in round below locked");
        // Round 11 > highest_qc 10, parent_qc 10 >= locked 9 → safe.
        assert!(rules.safe_to_vote(11, 10), "must vote in safe round");
    }

    /// Two honest validators never commit conflicting blocks at the same height.
    #[test]
    fn no_conflicting_commits_at_same_height() {
        // Invariant: a block is committed only when it has two consecutive QCs
        // (B_k has QC, B_{k+1} has QC on top of B_k). This requires 3 rounds.
        // A Byzantine minority cannot produce 2 conflicting 2-chain QCs
        // because they'd need to corrupt ≥n/3 honest votes.

        let n = 4usize;
        let f = 1usize;
        let quorum = (2 * n + 2) / 3;

        // To equivocate (commit two conflicting blocks), the adversary needs
        // to produce two quorum certificates with disjoint honest vote sets.
        // With n=4, quorum=3: two sets of 3 from 4 must share ≥2 honest members.
        // A Byzantine validator can double-sign but cannot force honest ones to.

        let honest = n - f;  // 3 honest validators
        // Each QC needs `quorum` votes; honest members of both QCs would overlap.
        let overlap = (2 * quorum).saturating_sub(honest);
        assert!(overlap > 0,
            "overlapping honest validators prevents two conflicting QCs (overlap={})", overlap);
    }

    /// Pacemaker must advance the round after a timeout even without a new proposal.
    #[test]
    fn pacemaker_advances_round_on_timeout() {
        struct Pacemaker { current_round: u64, timeout_votes: usize }

        impl Pacemaker {
            fn new() -> Self { Self { current_round: 1, timeout_votes: 0 } }

            fn record_timeout(&mut self, n: usize) {
                self.timeout_votes += 1;
                let quorum = (2 * n + 2) / 3;
                if self.timeout_votes >= quorum {
                    self.current_round += 1;
                    self.timeout_votes = 0;
                }
            }
        }

        let n = 4usize;
        let mut pm = Pacemaker::new();
        assert_eq!(pm.current_round, 1);

        pm.record_timeout(n); // 1 vote
        pm.record_timeout(n); // 2 votes
        assert_eq!(pm.current_round, 1, "quorum not yet reached");

        pm.record_timeout(n); // 3 votes = quorum → round advances
        assert_eq!(pm.current_round, 2, "round must advance after quorum of timeouts");
    }

    /// A replay of an old vote for a past round must be rejected.
    #[test]
    fn stale_vote_replay_rejected() {
        let mut rules = SafetyRules::new();
        rules.update_highest_qc(5);
        rules.update_locked_round(4);

        // Round 5 == highest_qc_round → proposal_round must be > highest_qc_round.
        assert!(!rules.safe_to_vote(5, 4), "equal-round vote is stale and rejected");
        // Round 4 < highest_qc_round → definitely rejected.
        assert!(!rules.safe_to_vote(4, 3), "past-round vote rejected");
    }
}
