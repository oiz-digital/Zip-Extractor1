//! Dispute handling — when a challenger contests a proposal.

use serde::{Serialize, Deserialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DisputeOutcome {
    /// Proposer was correct — challenger loses bond
    ProposerWon { correct_price: i128 },
    /// Challenger was correct — proposer loses bond
    ChallengerWon { correct_price: i128 },
    /// DVM could not reach consensus — both get bonds back minus fee
    NoContest,
}

/// An active dispute between proposer and challenger.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Dispute {
    pub request_id:      [u8; 32],
    pub proposer:        [u8; 20],
    pub challenger:      [u8; 20],
    pub proposed_price:  i128,
    pub challenger_bond: u128,
    pub disputed_at:     u64,
    pub outcome:         Option<DisputeOutcome>,
}

impl Dispute {
    pub fn new(
        request_id:      [u8; 32],
        proposer:        [u8; 20],
        challenger:      [u8; 20],
        proposed_price:  i128,
        challenger_bond: u128,
        now:             u64,
    ) -> Self {
        tracing::info!(
            request = hex::encode(request_id),
            proposer = hex::encode(proposer),
            challenger = hex::encode(challenger),
            "Optimistic oracle dispute raised"
        );
        Self { request_id, proposer, challenger, proposed_price, challenger_bond, disputed_at: now, outcome: None }
    }

    /// Resolve the dispute with a DVM-voted price.
    pub fn resolve(&mut self, dvm_price: i128, proposer_correct: bool) {
        let outcome = if proposer_correct {
            DisputeOutcome::ProposerWon { correct_price: dvm_price }
        } else {
            DisputeOutcome::ChallengerWon { correct_price: dvm_price }
        };
        tracing::info!(
            request = hex::encode(self.request_id),
            outcome = ?outcome,
            "Optimistic oracle dispute resolved"
        );
        self.outcome = Some(outcome);
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> [u8; 32] { [0xbbu8; 32] }

    #[test]
    fn new_dispute_has_no_outcome() {
        let d = Dispute::new(req(), [1u8; 20], [2u8; 20], 3500_00000000, 1_000_000_000, 1_700_000_000);
        assert!(d.outcome.is_none());
    }

    #[test]
    fn resolve_proposer_wins() {
        let mut d = Dispute::new(req(), [1u8; 20], [2u8; 20], 100, 0, 0);
        d.resolve(100, true);
        assert!(matches!(d.outcome, Some(DisputeOutcome::ProposerWon { .. })));
    }

    #[test]
    fn resolve_challenger_wins() {
        let mut d = Dispute::new(req(), [1u8; 20], [2u8; 20], 100, 0, 0);
        d.resolve(200, false);
        assert!(matches!(d.outcome, Some(DisputeOutcome::ChallengerWon { correct_price: 200 })));
    }

    #[test]
    fn dispute_stores_challenger_bond() {
        let bond = 5_000_000_000u128;
        let d = Dispute::new(req(), [1u8; 20], [2u8; 20], 0, bond, 0);
        assert_eq!(d.challenger_bond, bond);
    }

    #[test]
    fn dispute_stores_disputed_at() {
        let now = 1_700_000_000u64;
        let d = Dispute::new(req(), [1u8; 20], [2u8; 20], 0, 0, now);
        assert_eq!(d.disputed_at, now);
    }
}
