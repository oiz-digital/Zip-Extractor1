//! Block history retention policy.

use std::collections::VecDeque;
use zbx_primitives::H256;

#[derive(Debug, Clone)]
pub struct HistoryConfig {
    pub keep_last_n: u64,
    pub archive: bool,
    pub pinned_blocks: Vec<u64>,
}
impl Default for HistoryConfig {
    fn default() -> Self { Self { keep_last_n: 128, archive: false, pinned_blocks: vec![] } }
}

#[derive(Debug, Clone)]
pub struct HistoryEntry { pub block: u64, pub state_root: H256, pub pinned: bool }

pub struct HistoryManager { pub config: HistoryConfig, pub history: VecDeque<HistoryEntry> }

impl HistoryManager {
    pub fn new(config: HistoryConfig) -> Self { Self { config, history: VecDeque::new() } }

    pub fn push(&mut self, block: u64, root: H256) {
        self.history.push_back(HistoryEntry { block, state_root: root, pinned: false });
        self.trim();
    }

    pub fn pin(&mut self, block: u64) {
        if let Some(e) = self.history.iter_mut().find(|e| e.block == block) { e.pinned = true; }
        else { self.config.pinned_blocks.push(block); }
    }

    pub fn can_prune(&self, block: u64, head: u64) -> bool {
        !self.config.archive && !self.config.pinned_blocks.contains(&block)
            && head.saturating_sub(block) > self.config.keep_last_n
    }

    fn trim(&mut self) {
        while self.history.len() > self.config.keep_last_n as usize * 2 {
            if self.history.front().map(|e| e.pinned).unwrap_or(true) { break; }
            self.history.pop_front();
        }
    }
}