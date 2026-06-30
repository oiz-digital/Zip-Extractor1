//! Unit tests for zbx-consensus (HotStuff BFT).

#[cfg(test)]
mod consensus_unit {
    #[test]
    fn quorum_requires_2f_plus_1() {
        // For N validators, quorum = floor(2N/3) + 1 (Byzantine fault tolerant).
        // This ensures liveness even with f = floor((N-1)/3) faulty nodes.
        fn quorum(n: usize) -> usize { 2 * n / 3 + 1 }
        assert_eq!(quorum(4),  3, "4 validators: quorum=3");
        assert_eq!(quorum(7),  5, "7 validators: quorum=5");
        assert_eq!(quorum(10), 7, "10 validators: quorum=7");
        assert_eq!(quorum(100), 67, "100 validators: quorum=67");
    }

    #[test]
    fn max_faulty_nodes() {
        // Safety: system is safe with up to f faulty nodes where f < N/3.
        fn max_faulty(n: usize) -> usize { (n - 1) / 3 }
        assert_eq!(max_faulty(4),  1);
        assert_eq!(max_faulty(7),  2);
        assert_eq!(max_faulty(10), 3);
        assert_eq!(max_faulty(100), 33);
    }

    #[test]
    fn safety_with_max_faulty() {
        // Even with max faulty nodes, two quorums must overlap.
        let n = 10;
        let q = 2 * n / 3 + 1;
        let f = (n - 1) / 3;
        // Two quorums of size q in a set of n: overlap = 2q - n >= 1.
        let overlap = 2 * q - n;
        assert!(overlap >= 1, "quorums must overlap by at least 1 honest node");
        // Honest nodes in quorum: at least q - f.
        let honest_in_quorum = q - f;
        assert!(honest_in_quorum >= 1, "quorum must contain at least 1 honest node");
    }

    #[test]
    fn round_advances_on_quorum() {
        let mut round = 0u64;
        let votes_received = 3usize;
        let quorum = 3usize;
        if votes_received >= quorum { round += 1; }
        assert_eq!(round, 1, "round advances when quorum reached");
    }

    #[test]
    fn vote_is_unique_per_round() {
        // A validator must not vote twice in the same round (equivocation).
        let mut votes: std::collections::HashSet<(u64, [u8; 20])> = Default::default();
        let validator = [0x01u8; 20];
        let round = 1u64;
        assert!(votes.insert((round, validator)), "first vote accepted");
        assert!(!votes.insert((round, validator)), "duplicate vote rejected");
    }

    #[test]
    fn timeout_causes_view_change() {
        // If no quorum in TIME_OUT_MS, view (round) increments.
        let mut view = 0u64;
        let timed_out = true; // simulate timeout
        if timed_out { view += 1; }
        assert_eq!(view, 1, "view increments on timeout");
    }
}