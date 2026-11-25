//! In-memory [`LedgerView`] for tests, tools and the mempool's scratch state.

use std::collections::{BTreeMap, HashMap};

use elements::OutPoint;

use crate::types::{BlockDiff, LedgerView, UtxoEntry};

#[derive(Clone, Debug, Default)]
pub struct MemoryLedger {
    utxos: HashMap<OutPoint, UtxoEntry>,
    times: BTreeMap<u64, u64>,
}

impl MemoryLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, outpoint: OutPoint, entry: UtxoEntry) {
        self.utxos.insert(outpoint, entry);
    }

    /// Records the timestamp of the block at `height`.
    pub fn set_block_time(&mut self, height: u64, time: u64) {
        self.times.insert(height, time);
    }

    /// Applies a block's state change.
    pub fn apply(&mut self, diff: &BlockDiff) {
        for (op, _) in &diff.spent {
            self.utxos.remove(op);
        }
        for (op, entry) in &diff.created {
            self.utxos.insert(*op, entry.clone());
        }
    }

    pub fn len(&self) -> usize {
        self.utxos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.utxos.is_empty()
    }
}

impl LedgerView for MemoryLedger {
    fn utxo(&self, outpoint: &OutPoint) -> Option<UtxoEntry> {
        self.utxos.get(outpoint).cloned()
    }

    fn block_time(&self, height: u64) -> u64 {
        self.times.get(&height).copied().unwrap_or(0)
    }
}
