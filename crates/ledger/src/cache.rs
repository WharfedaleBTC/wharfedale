//! Cache of expensive verification results (amount proofs, signatures, Simplicity).
//!
//! Keyed by wtxid: a wtxid commits to every prevout, and an outpoint's output never changes,
//! so a result is valid for any chain state in which the transaction's inputs exist. One cache
//! serves one chain (the genesis hash is part of every sighash but not of the key).
//!
//! An entry is written only after the whole transaction passed (`check_tx`), so invalid
//! transactions never occupy it. Bounded: the oldest entries are evicted first.

use std::collections::{HashSet, VecDeque};
use std::sync::Mutex;

use elements::Wtxid;

/// Default capacity (entries): about 10 MB.
pub const DEFAULT_CAPACITY: usize = 100_000;

#[derive(Debug)]
pub struct VerifyCache {
    inner: Mutex<Inner>,
    capacity: usize,
}

#[derive(Debug, Default)]
struct Inner {
    set: HashSet<Wtxid>,
    order: VecDeque<Wtxid>,
}

impl Default for VerifyCache {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }
}

impl VerifyCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self { inner: Mutex::new(Inner::default()), capacity: capacity.max(1) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether `wtxid`'s proofs and spends are known valid.
    pub fn contains(&self, wtxid: Wtxid) -> bool {
        self.lock().set.contains(&wtxid)
    }

    pub(crate) fn insert(&self, wtxid: Wtxid) {
        let mut inner = self.lock();
        if inner.set.insert(wtxid) {
            inner.order.push_back(wtxid);
            while inner.order.len() > self.capacity {
                if let Some(old) = inner.order.pop_front() {
                    inner.set.remove(&old);
                }
            }
        }
    }

    pub fn len(&self) -> usize {
        self.lock().set.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
