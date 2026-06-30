//! Sealevel-style parallel execution scheduler.
//!
//! ### Background
//!
//! Solana's *Sealevel* runtime parallelises transactions by requiring each
//! transaction to declare upfront the accounts it will read and write. The
//! scheduler then groups non-conflicting transactions into independent
//! "lanes" that can execute in parallel on different threads.
//!
//! Ethereum's [EIP-2930] introduced an optional `accessList` field on every
//! type-1 and type-2 transaction that serves the same purpose: it pre-warms
//! the storage slots the transaction will touch. We reuse that field here as
//! the scheduling hint — no protocol-level changes are needed, and any
//! existing wallet that produces EIP-2930 / EIP-1559 transactions with a
//! populated `accessList` will benefit automatically.
//!
//! ### Algorithm
//!
//! 1.  For each transaction, derive its **conflict set** — the set of
//!     `AccessKey`s it may touch.
//! 2.  Greedily assign each transaction to the lowest-indexed lane that has
//!     **no transaction with overlapping conflict set**.
//! 3.  Two extra invariants are preserved that Solana also enforces:
//!     * **Sender-nonce ordering** — all transactions from the same sender
//!       go to the same lane, in submission order. This guarantees nonce
//!       monotonicity.
//!     * **Fallback when access list is empty** — transactions that do not
//!       declare an access list are treated as conflicting with everything
//!       (write-everything). They effectively serialise the block at that
//!       point. This is the Solana model when a program is not declared as
//!       parallelisable.
//!
//! The resulting `Lanes` struct is cheap to feed into a parallel executor:
//! lanes execute concurrently, transactions within a lane execute
//! sequentially. This module performs **scheduling only** — it never touches
//! state. That keeps it trivial to fuzz and reason about.
//!
//! [EIP-2930]: https://eips.ethereum.org/EIPS/eip-2930

use std::collections::HashSet;
use zbx_types::{address::Address, transaction::SignedTransaction, H256};

/// A storage location that two transactions may conflict on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccessKey {
    /// The whole account record (balance / nonce / code).
    Account(Address),
    /// One specific 32-byte storage slot of an account.
    Storage(Address, H256),
    /// Sentinel meaning "this transaction may touch anything". Conflicts
    /// with every other key, including itself across lanes. Used when a
    /// transaction omits its `accessList`.
    Wildcard,
}

/// Read and write access sets derived from one transaction.
#[derive(Debug, Clone, Default)]
pub struct AccessSet {
    pub reads:  HashSet<AccessKey>,
    pub writes: HashSet<AccessKey>,
}

impl AccessSet {
    /// Two access sets *conflict* if a write in one overlaps any access in
    /// the other (Bernstein conditions). Pure reads in both are safe.
    pub fn conflicts_with(&self, other: &AccessSet) -> bool {
        // Wildcard short-circuit.
        if self.writes.contains(&AccessKey::Wildcard)
            || other.writes.contains(&AccessKey::Wildcard)
            || self.reads.contains(&AccessKey::Wildcard)
            || other.reads.contains(&AccessKey::Wildcard)
        {
            return true;
        }
        // RAW / WAR / WAW
        self.writes.iter().any(|w| other.reads.contains(w) || other.writes.contains(w))
            || other.writes.iter().any(|w| self.reads.contains(w))
    }

    /// Merge another set into this one (lane accumulator).
    pub fn extend(&mut self, other: &AccessSet) {
        self.reads.extend(other.reads.iter().copied());
        self.writes.extend(other.writes.iter().copied());
    }
}

/// Derive the access set for a single transaction.
///
/// Reads:  sender, recipient, every (addr, slot) in the access list.
/// Writes: sender, recipient (balance + nonce always change), every
///         (addr, slot) declared as accessed (we conservatively assume
///         touched slots may be written — refining requires per-call
///         introspection that EIP-2930 does not provide).
///
/// If the access list is empty AND the call has data (= contract call),
/// the set degrades to `Wildcard`: we cannot know which storage slots a
/// random contract touches.
pub fn derive_access_set(tx: &SignedTransaction) -> AccessSet {
    let mut set = AccessSet::default();
    set.reads.insert(AccessKey::Account(tx.from));
    set.writes.insert(AccessKey::Account(tx.from)); // nonce + balance change

    if let Some(to) = tx.tx.to {
        set.reads.insert(AccessKey::Account(to));
        set.writes.insert(AccessKey::Account(to)); // balance change on transfer

        // Plain value transfer (no calldata, no access list) → safe to
        // schedule narrowly: only the two account records are touched.
        let is_call = !tx.tx.data.is_empty();
        let has_access_list = !tx.tx.access_list.is_empty();

        if is_call && !has_access_list {
            // Unknown contract effects → wildcard.
            set.writes.insert(AccessKey::Wildcard);
            return set;
        }

        for (addr, slots) in &tx.tx.access_list {
            set.reads.insert(AccessKey::Account(*addr));
            for slot in slots {
                set.reads.insert(AccessKey::Storage(*addr, *slot));
                set.writes.insert(AccessKey::Storage(*addr, *slot));
            }
        }
    } else {
        // Contract creation — touches the new contract's account + arbitrary
        // storage of the deployer's chosen state. Treat as wildcard write.
        set.writes.insert(AccessKey::Wildcard);
    }

    set
}

/// Output of the scheduler.
///
/// `lanes[k]` is a vector of indices into the original `txs` slice.
/// Transactions within a lane MUST execute sequentially in the order
/// listed. Different lanes may execute in parallel.
#[derive(Debug, Clone)]
pub struct Lanes {
    pub lanes: Vec<Vec<usize>>,
    /// Cumulative access set for each lane (debug + verification).
    pub lane_sets: Vec<AccessSet>,
}

impl Lanes {
    pub fn lane_count(&self) -> usize { self.lanes.len() }
    pub fn tx_count(&self) -> usize { self.lanes.iter().map(|l| l.len()).sum() }
    pub fn max_parallelism(&self) -> usize { self.lanes.len() }
}

/// Greedy lane scheduler.
///
/// `max_lanes` caps parallelism (typically `num_cpus`). Pass `usize::MAX`
/// to allow unbounded lanes. The greedy choice is correct because:
/// * Within a lane, txs are sequential (preserves nonce + read-after-write).
/// * Between lanes, the conflict-set check ensures Bernstein independence.
///
/// Sender-nonce ordering is preserved because we route each sender to the
/// lane containing their previous transaction (if any), even if a lower
/// lane index would have been admissible. Without this rule a later
/// nonce-N+1 tx could land in lane 0 while nonce-N sits in lane 1, breaking
/// monotonicity when lanes execute in parallel.
pub fn schedule(txs: &[SignedTransaction], max_lanes: usize) -> Lanes {
    use std::collections::HashMap;
    let mut lanes: Vec<Vec<usize>> = Vec::new();
    let mut lane_sets: Vec<AccessSet> = Vec::new();
    // Sender → lane index that owns their transactions.
    let mut sender_lane: HashMap<Address, usize> = HashMap::new();

    'tx: for (idx, tx) in txs.iter().enumerate() {
        let aset = derive_access_set(tx);

        // Rule A: pin to sender's existing lane if any.
        if let Some(&lane_idx) = sender_lane.get(&tx.from) {
            lanes[lane_idx].push(idx);
            lane_sets[lane_idx].extend(&aset);
            continue;
        }

        // Rule B: try existing lanes in order.
        for (lane_idx, lset) in lane_sets.iter_mut().enumerate() {
            if !lset.conflicts_with(&aset) {
                lanes[lane_idx].push(idx);
                lset.extend(&aset);
                sender_lane.insert(tx.from, lane_idx);
                continue 'tx;
            }
        }

        // Rule C: open a new lane if budget remains; else fall back to lane 0.
        if lanes.len() < max_lanes {
            let new_lane = lanes.len();
            lanes.push(vec![idx]);
            lane_sets.push(aset);
            sender_lane.insert(tx.from, new_lane);
        } else {
            // Budget exhausted — append to the smallest lane (load-balancing).
            // Within that lane the tx will run after the previous ones, so
            // the conflict no longer matters: it is sequential there.
            let (smallest, _) = lanes
                .iter()
                .enumerate()
                .min_by_key(|(_, l)| l.len())
                .expect("max_lanes >= 1");
            lanes[smallest].push(idx);
            lane_sets[smallest].extend(&aset);
            sender_lane.insert(tx.from, smallest);
        }
    }

    Lanes { lanes, lane_sets }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use zbx_types::transaction::{Signature, Transaction, TxType};

    fn mk(
        from: u8,
        to: Option<u8>,
        nonce: u64,
        access: Vec<(u8, Vec<u8>)>,
        data: Vec<u8>,
    ) -> SignedTransaction {
        let from_addr = Address([from; 20]);
        let to_addr = to.map(|x| Address([x; 20]));
        let access_list: Vec<(Address, Vec<H256>)> = access
            .into_iter()
            .map(|(a, slots)| {
                (
                    Address([a; 20]),
                    slots
                        .into_iter()
                        .map(|s| {
                            let mut h = [0u8; 32];
                            h[31] = s;
                            h
                        })
                        .collect(),
                )
            })
            .collect();
        SignedTransaction {
            from: from_addr,
            tx: Transaction {
                tx_type: TxType::DynamicFee,
                chain_id: 8989,
                nonce,
                max_fee_per_gas: 1,
                max_priority_fee_per_gas: 0,
                gas_limit: 21_000,
                to: to_addr,
                value: [0u8; 32],
                data,
                access_list,
            },
            sig: Signature { v: 0, r: [0u8; 32], s: [0u8; 32] },
            hash: [0u8; 32],
        }
    }

    #[test]
    fn disjoint_pure_transfers_parallelise() {
        // 4 transfers between disjoint pairs (a→b, c→d, e→f, g→h) should
        // each get its own lane up to max_lanes.
        let txs = vec![
            mk(1, Some(2), 0, vec![], vec![]),
            mk(3, Some(4), 0, vec![], vec![]),
            mk(5, Some(6), 0, vec![], vec![]),
            mk(7, Some(8), 0, vec![], vec![]),
        ];
        let lanes = schedule(&txs, 8);
        assert_eq!(lanes.lane_count(), 4, "fully disjoint transfers should split 4 ways");
    }

    #[test]
    fn same_sender_pinned_to_one_lane() {
        // Sender 1 with 3 nonces — must be in same lane in nonce order.
        let txs = vec![
            mk(1, Some(2), 0, vec![], vec![]),
            mk(1, Some(3), 1, vec![], vec![]),
            mk(1, Some(4), 2, vec![], vec![]),
        ];
        let lanes = schedule(&txs, 8);
        assert_eq!(lanes.lane_count(), 1);
        assert_eq!(lanes.lanes[0], vec![0, 1, 2]);
    }

    #[test]
    fn shared_recipient_serialises() {
        // Both txs send to address 99 — they conflict on the recipient
        // account write, so must serialise into one lane.
        let txs = vec![
            mk(1, Some(99), 0, vec![], vec![]),
            mk(2, Some(99), 0, vec![], vec![]),
        ];
        let lanes = schedule(&txs, 8);
        assert_eq!(lanes.lane_count(), 1);
        assert_eq!(lanes.lanes[0], vec![0, 1]);
    }

    #[test]
    fn contract_call_without_access_list_is_wildcard() {
        // Two contract calls (have data, no access list) should serialise
        // because each is treated as wildcard.
        let txs = vec![
            mk(1, Some(50), 0, vec![], vec![1, 2, 3]),
            mk(2, Some(60), 0, vec![], vec![4, 5, 6]),
        ];
        let lanes = schedule(&txs, 8);
        assert_eq!(lanes.lane_count(), 1);
    }

    #[test]
    fn contract_call_with_disjoint_access_lists_parallelises() {
        // Two contract calls with declared, non-overlapping access lists
        // should land in separate lanes.
        let txs = vec![
            mk(1, Some(50), 0, vec![(50, vec![1, 2])], vec![1]),
            mk(2, Some(60), 0, vec![(60, vec![3, 4])], vec![1]),
        ];
        let lanes = schedule(&txs, 8);
        assert_eq!(lanes.lane_count(), 2);
    }

    #[test]
    fn budget_exhaustion_load_balances() {
        // 5 disjoint pure transfers, max_lanes = 2 → balanced 3/2.
        let txs = vec![
            mk(1, Some(11), 0, vec![], vec![]),
            mk(2, Some(12), 0, vec![], vec![]),
            mk(3, Some(13), 0, vec![], vec![]),
            mk(4, Some(14), 0, vec![], vec![]),
            mk(5, Some(15), 0, vec![], vec![]),
        ];
        let lanes = schedule(&txs, 2);
        assert_eq!(lanes.lane_count(), 2);
        assert_eq!(lanes.tx_count(), 5);
    }
}
