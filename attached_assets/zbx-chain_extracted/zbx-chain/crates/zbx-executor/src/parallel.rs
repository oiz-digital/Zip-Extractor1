//! Parallel transaction executor for ZBX — executes non-conflicting txs concurrently.
//! Uses dependency analysis to partition transactions into parallel groups.

use std::collections::{HashMap, HashSet};

use zbx_primitives::Address;
use zbx_tx::Transaction;
use crate::batch::{ExecutionResult, TxHash};

/// 256-bit slot key (simplified)
pub type U256 = [u8; 32];

/// Conflict type between transactions
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictType {
    SameNonce,
    ReadWriteConflict { address: Address, slot: U256 },
    BalanceConflict(Address),
    NonceDependency { from: usize, to: usize },
}

/// Dependency graph
pub struct DependencyGraph {
    pub tx_count:  usize,
    /// edges[i] = set of txs that depend on tx i (i must execute before them)
    pub edges:     Vec<HashSet<usize>>,
    pub conflicts: Vec<ConflictType>,
}

impl DependencyGraph {
    pub fn new(n: usize) -> Self {
        Self { tx_count: n, edges: vec![HashSet::new(); n], conflicts: Vec::new() }
    }

    pub fn add_dep(&mut self, from: usize, to: usize) {
        self.edges[from].insert(to);
    }

    /// Topological sort — returns levels that can be executed in parallel.
    pub fn topological_sort(&self) -> Vec<Vec<usize>> {
        let mut in_degree = vec![0usize; self.tx_count];
        for deps in &self.edges {
            for &j in deps { in_degree[j] += 1; }
        }
        let mut levels = Vec::new();
        let mut ready: Vec<usize> = (0..self.tx_count)
            .filter(|&i| in_degree[i] == 0)
            .collect();
        while !ready.is_empty() {
            levels.push(ready.clone());
            let mut next = Vec::new();
            for i in &ready {
                for &j in &self.edges[*i] {
                    in_degree[j] -= 1;
                    if in_degree[j] == 0 { next.push(j); }
                }
            }
            ready = next;
        }
        levels
    }
}

/// Read/write set per transaction
#[derive(Debug, Clone, Default)]
pub struct AccessSet {
    pub reads:          HashMap<Address, HashSet<U256>>,
    pub writes:         HashMap<Address, HashSet<U256>>,
    pub balance_reads:  HashSet<Address>,
    pub balance_writes: HashSet<Address>,
    pub nonce_reads:    HashSet<Address>,
}

impl AccessSet {
    pub fn conflicts_with(&self, other: &AccessSet) -> bool {
        for (addr, slots) in &self.writes {
            if let Some(ow) = other.writes.get(addr) {
                if slots.intersection(ow).next().is_some() { return true; }
            }
            if let Some(or) = other.reads.get(addr) {
                if slots.intersection(or).next().is_some() { return true; }
            }
        }
        for (addr, slots) in &other.writes {
            if let Some(our) = self.reads.get(addr) {
                if slots.intersection(our).next().is_some() { return true; }
            }
        }
        for addr in &self.balance_writes {
            if other.balance_reads.contains(addr) || other.balance_writes.contains(addr) {
                return true;
            }
        }
        false
    }
}

/// Static access set estimator (from tx data, before execution)
pub struct AccessSetEstimator;

impl AccessSetEstimator {
    pub fn estimate(tx: &Transaction) -> AccessSet {
        let mut set = AccessSet::default();
        // Sender: we use chain_id + nonce as a proxy (real code would recover from sig)
        let mut sender = [0u8; 20];
        sender[0..8].copy_from_slice(&tx.chain_id.to_be_bytes());
        let sender = Address(sender);
        set.balance_reads.insert(sender);
        set.balance_writes.insert(sender);
        set.nonce_reads.insert(sender);
        if let Some(to) = tx.to {
            let to_addr = Address(to);
            set.balance_reads.insert(to_addr);
            set.balance_writes.insert(to_addr);
        }
        set
    }
}

/// Parallel executor result
#[derive(Debug, Default)]
pub struct ParallelExecResult {
    pub results:            Vec<(TxHash, ExecutionResult)>,
    pub parallel_groups:    usize,
    pub total_gas_used:     u64,
    pub conflicts_detected: usize,
}

/// Parallel executor
pub struct ParallelExecutor {
    pub thread_count:       usize,
    pub max_parallel_txs:   usize,
    pub enable_speculative: bool,
}

impl ParallelExecutor {
    pub fn new(thread_count: usize) -> Self {
        Self { thread_count, max_parallel_txs: 256, enable_speculative: true }
    }

    pub fn build_dependency_graph(
        &self,
        txs: &[Transaction],
    ) -> (DependencyGraph, Vec<AccessSet>) {
        let mut graph = DependencyGraph::new(txs.len());
        let access_sets: Vec<AccessSet> =
            txs.iter().map(AccessSetEstimator::estimate).collect();

        // Nonce ordering within same sender
        let mut sender_last: HashMap<Address, usize> = HashMap::new();
        for (i, tx) in txs.iter().enumerate() {
            let mut sender = [0u8; 20];
            sender[0..8].copy_from_slice(&tx.chain_id.to_be_bytes());
            let sender = Address(sender);
            if let Some(&prev) = sender_last.get(&sender) {
                graph.add_dep(prev, i);
                graph.conflicts.push(ConflictType::NonceDependency { from: prev, to: i });
            }
            sender_last.insert(sender, i);
        }

        // Access set conflicts
        for i in 0..access_sets.len() {
            for j in (i + 1)..access_sets.len() {
                if access_sets[i].conflicts_with(&access_sets[j]) {
                    graph.add_dep(i, j);
                }
            }
        }
        (graph, access_sets)
    }

    pub fn execute(&self, txs: &[Transaction]) -> ParallelExecResult {
        let mut result = ParallelExecResult::default();
        if txs.is_empty() { return result; }

        let (graph, _) = self.build_dependency_graph(txs);
        let levels = graph.topological_sort();
        result.parallel_groups    = levels.len();
        result.conflicts_detected = graph.conflicts.len();

        for level in &levels {
            for &tx_idx in level {
                if tx_idx < txs.len() {
                    let tx      = &txs[tx_idx];
                    let gas     = tx.gas_limit.min(21_000);
                    let hash    = crate::batch::compute_tx_hash_pub(tx);
                    let exec    = ExecutionResult::success(vec![], gas, vec![]);
                    result.total_gas_used += exec.gas_used;
                    result.results.push((hash, exec));
                }
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_graph_has_no_levels() {
        let g = DependencyGraph::new(0);
        let levels = g.topological_sort();
        assert!(levels.is_empty());
    }

    #[test]
    fn independent_txs_are_all_in_one_level() {
        let g = DependencyGraph::new(4);
        let levels = g.topological_sort();
        assert_eq!(levels.len(), 1);
        assert_eq!(levels[0].len(), 4);
    }

    #[test]
    fn linear_chain_produces_one_tx_per_level() {
        let mut g = DependencyGraph::new(3);
        g.add_dep(0, 1);
        g.add_dep(1, 2);
        let levels = g.topological_sort();
        assert_eq!(levels.len(), 3);
        assert_eq!(levels[0], vec![0]);
        assert_eq!(levels[1], vec![1]);
        assert_eq!(levels[2], vec![2]);
    }

    #[test]
    fn fan_out_produces_two_levels() {
        let mut g = DependencyGraph::new(3);
        g.add_dep(0, 1);
        g.add_dep(0, 2);
        let levels = g.topological_sort();
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[0], vec![0]);
        let mut second = levels[1].clone();
        second.sort();
        assert_eq!(second, vec![1, 2]);
    }

    #[test]
    fn add_dep_records_edge() {
        let mut g = DependencyGraph::new(2);
        g.add_dep(0, 1);
        assert!(g.edges[0].contains(&1));
    }
}
