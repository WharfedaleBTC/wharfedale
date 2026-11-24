//! Transaction pool, v1 policy: confirmed inputs only, first-seen (no replacement), ordered by
//! fee rate in the fee asset. Spec: `docs/src/mempool.md`.
#![forbid(unsafe_code)]

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use elements::{AssetId, OutPoint, Transaction, Txid, Wtxid};
use wharfedale_ledger::{check_tx, check_tx_stateless, ChainCtx, LedgerView, TxError, VerifyCache};

/// Default minimum fee rate: 10 fee-asset units per 1 000 vbytes (0.01 sat/vB for a
/// bitcoin-like fee asset; Liquid's is 0.1). Nodes set their own (`min_fee_per_kvb`).
pub const DEFAULT_MIN_FEE_PER_KVB: u64 = 10;

/// Admission policy (not consensus).
#[derive(Clone, Debug)]
pub struct Policy {
    /// Only fees in this asset count toward the fee rate.
    pub fee_asset: AssetId,
    /// Minimum fee per 1000 vbytes, in fee-asset units.
    pub min_fee_per_kvb: u64,
    /// Total vsize the pool may hold.
    pub max_pool_vsize: usize,
    /// Entries older than this are dropped.
    pub expiry: Duration,
    /// Remembered known-invalid wtxids.
    pub rejected_cache: usize,
}

impl Policy {
    pub fn new(fee_asset: AssetId) -> Self {
        Self {
            fee_asset,
            min_fee_per_kvb: DEFAULT_MIN_FEE_PER_KVB,
            max_pool_vsize: 300_000_000,
            expiry: Duration::from_secs(24 * 3600),
            rejected_cache: 10_000,
        }
    }
}

/// Where a transaction came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Submitted to this node's RPC: relayed to peers.
    Local,
    /// Received by gossip: not re-published (gossipsub relays).
    Gossip,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub tx: Arc<Transaction>,
    pub txid: Txid,
    pub wtxid: Wtxid,
    pub vsize: usize,
    /// Fee in the fee asset.
    pub fee: u64,
    pub time: Instant,
    pub source: Source,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AdmitError {
    #[error("already in the mempool")]
    Duplicate,
    #[error("previously rejected")]
    KnownInvalid,
    #[error("input {0} is missing or unconfirmed (v1 accepts confirmed inputs only)")]
    MissingInput(usize),
    #[error("input {0} conflicts with mempool transaction {1}")]
    Conflict(usize, Txid),
    #[error("fee {fee} below minimum {min} for {vsize} vbytes")]
    FeeTooLow { fee: u64, min: u64, vsize: usize },
    #[error("mempool full")]
    PoolFull,
    #[error("fee paid in asset {0}: only the fee asset is accepted")]
    FeeAsset(AssetId),
    #[error("node unavailable: {0}")]
    Unavailable(String),
    #[error("invalid: {0}")]
    Invalid(#[from] TxError),
}

/// A transaction that passed [`Mempool::precheck`]: verify it, then [`Mempool::insert`].
#[derive(Clone, Debug)]
pub struct Candidate {
    pub tx: Transaction,
    pub txid: Txid,
    pub wtxid: Wtxid,
    pub vsize: usize,
    pub fee: u64,
    pub source: Source,
}

/// Full consensus verification of a candidate (signatures, proofs, Simplicity); no pool
/// state needed, so it runs outside any lock.
pub fn verify<V: LedgerView>(c: &Candidate, view: &V, ctx: &ChainCtx, cache: &VerifyCache) -> Result<(), TxError> {
    check_tx(view, &c.tx, ctx, cache).map(|_| ())
}

/// Fee rate key: fee per vbyte scaled by 1000, then txid for a total order.
type OrderKey = (u64, Txid);

#[derive(Debug)]
pub struct Mempool {
    policy: Policy,
    txs: HashMap<Txid, Entry>,
    by_wtxid: HashSet<Wtxid>,
    spends: HashMap<OutPoint, Txid>,
    order: BTreeSet<OrderKey>,
    rejected: HashSet<Wtxid>,
    rejected_order: VecDeque<Wtxid>,
    vsize: usize,
}

impl Mempool {
    pub fn new(policy: Policy) -> Self {
        Self {
            policy,
            txs: HashMap::new(),
            by_wtxid: HashSet::new(),
            spends: HashMap::new(),
            order: BTreeSet::new(),
            rejected: HashSet::new(),
            rejected_order: VecDeque::new(),
            vsize: 0,
        }
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// Admission pipeline (`docs/src/mempool.md`): [`precheck`](Self::precheck), [`verify`],
    /// [`insert`](Self::insert). `view` is the committed state; `ctx` is the context of the
    /// next block (tip + 1). Callers sharing the pool behind a lock run the three steps
    /// separately so verification happens without the lock (`NodeState::admit`).
    pub fn admit<V: LedgerView>(
        &mut self,
        tx: Transaction,
        source: Source,
        view: &V,
        ctx: &ChainCtx,
        cache: &VerifyCache,
        now: Instant,
    ) -> Result<Txid, AdmitError> {
        let candidate = self.precheck(tx, source, view)?;
        if let Err(e) = verify(&candidate, view, ctx, cache) {
            self.record_failure(candidate.wtxid, &e);
            return Err(e.into());
        }
        self.insert(candidate, view, now)
    }

    /// Cheap checks, before any proof or signature: duplicates, known-invalid, stateless
    /// rules, fee policy, conflicts, confirmed inputs, room in the pool.
    pub fn precheck<V: LedgerView>(&mut self, tx: Transaction, source: Source, view: &V) -> Result<Candidate, AdmitError> {
        let txid = tx.txid();
        let wtxid = tx.wtxid();
        if self.txs.contains_key(&txid) || self.by_wtxid.contains(&wtxid) {
            return Err(AdmitError::Duplicate);
        }
        if self.rejected.contains(&wtxid) {
            return Err(AdmitError::KnownInvalid);
        }
        let fees = match check_tx_stateless(&tx) {
            Ok(fees) => fees,
            Err(e) => {
                self.reject(wtxid);
                return Err(e.into());
            }
        };
        // Policy: fees only in the fee asset (the coinbase claims nothing else).
        if let Some(other) = fees.keys().find(|a| **a != self.policy.fee_asset) {
            self.reject(wtxid);
            return Err(AdmitError::FeeAsset(*other));
        }
        let vsize = tx.weight().div_ceil(4);
        let fee = fees.get(&self.policy.fee_asset).copied().unwrap_or(0);
        let min = (self.policy.min_fee_per_kvb * vsize as u64).div_ceil(1000);
        if fee < min {
            return Err(AdmitError::FeeTooLow { fee, min, vsize });
        }
        self.check_inputs(&tx, view)?;
        if self.victims(order_key(fee, vsize, txid), vsize).is_none() {
            return Err(AdmitError::PoolFull);
        }
        Ok(Candidate { tx, txid, wtxid, vsize, fee, source })
    }

    /// Remembers `wtxid` as invalid unless `error` may pass later or is a local fault.
    pub fn record_failure(&mut self, wtxid: Wtxid, error: &TxError) {
        let transient = matches!(
            error,
            TxError::NotFinal
                | TxError::SequenceLocked
                | TxError::ImmatureCoinbase(_)
                | TxError::MissingInput(..)
                | TxError::SimplicityNodeFault(..)
        );
        if !transient {
            self.reject(wtxid);
        }
    }

    /// Adds a verified candidate. Re-checks what may have changed since `precheck` (another
    /// admission, a committed block), then evicts cheaper entries if needed.
    pub fn insert<V: LedgerView>(&mut self, c: Candidate, view: &V, now: Instant) -> Result<Txid, AdmitError> {
        if self.txs.contains_key(&c.txid) || self.by_wtxid.contains(&c.wtxid) {
            return Err(AdmitError::Duplicate);
        }
        self.check_inputs(&c.tx, view)?;
        let key = order_key(c.fee, c.vsize, c.txid);
        let victims = self.victims(key, c.vsize).ok_or(AdmitError::PoolFull)?;
        for (_, victim) in victims {
            self.remove(&victim);
        }
        for input in &c.tx.input {
            self.spends.insert(input.previous_output, c.txid);
        }
        self.order.insert(key);
        self.by_wtxid.insert(c.wtxid);
        self.vsize += c.vsize;
        let Candidate { tx, txid, wtxid, vsize, fee, source } = c;
        self.txs.insert(txid, Entry { tx: Arc::new(tx), txid, wtxid, vsize, fee, time: now, source });
        Ok(txid)
    }

    fn check_inputs<V: LedgerView>(&self, tx: &Transaction, view: &V) -> Result<(), AdmitError> {
        for (i, input) in tx.input.iter().enumerate() {
            if let Some(other) = self.spends.get(&input.previous_output) {
                return Err(AdmitError::Conflict(i, *other));
            }
            if view.utxo(&input.previous_output).is_none() {
                return Err(AdmitError::MissingInput(i));
            }
        }
        Ok(())
    }

    /// Entries to evict (strictly cheaper, lowest fee rate first) so `vsize` more fits; `None`
    /// if it can't fit.
    fn victims(&self, key: OrderKey, vsize: usize) -> Option<Vec<OrderKey>> {
        let max = self.policy.max_pool_vsize;
        let mut freed = 0;
        let mut victims = Vec::new();
        for k in &self.order {
            if self.vsize - freed + vsize <= max || *k >= key {
                break;
            }
            freed += self.txs[&k.1].vsize;
            victims.push(*k);
        }
        (self.vsize - freed + vsize <= max).then_some(victims)
    }

    /// Removes the block's transactions and every pool transaction spending an outpoint the
    /// block spent.
    pub fn remove_for_block(&mut self, txs: &[Transaction]) {
        for tx in txs {
            self.remove(&tx.txid());
            for input in &tx.input {
                if let Some(other) = self.spends.get(&input.previous_output).copied() {
                    self.remove(&other);
                }
            }
        }
    }

    /// Drops entries older than the policy expiry.
    pub fn expire(&mut self, now: Instant) {
        let expiry = self.policy.expiry;
        let old: Vec<Txid> =
            self.txs.values().filter(|e| now.duration_since(e.time) > expiry).map(|e| e.txid).collect();
        for txid in old {
            self.remove(&txid);
        }
    }

    /// Highest fee rate first, until `max_weight`.
    pub fn template(&self, max_weight: usize) -> Vec<Arc<Transaction>> {
        let mut weight = 0;
        let mut out = Vec::new();
        for (_, txid) in self.order.iter().rev() {
            let entry = &self.txs[txid];
            let w = entry.tx.weight();
            if weight + w > max_weight {
                continue;
            }
            weight += w;
            out.push(entry.tx.clone());
        }
        out
    }

    pub fn remove(&mut self, txid: &Txid) -> Option<Entry> {
        let entry = self.txs.remove(txid)?;
        for input in &entry.tx.input {
            if self.spends.get(&input.previous_output) == Some(txid) {
                self.spends.remove(&input.previous_output);
            }
        }
        self.order.remove(&order_key(entry.fee, entry.vsize, entry.txid));
        self.by_wtxid.remove(&entry.wtxid);
        self.vsize -= entry.vsize;
        Some(entry)
    }

    pub fn get(&self, txid: &Txid) -> Option<&Entry> {
        self.txs.get(txid)
    }

    pub fn txids(&self) -> Vec<Txid> {
        self.order.iter().rev().map(|(_, t)| *t).collect()
    }

    pub fn len(&self) -> usize {
        self.txs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.txs.is_empty()
    }

    /// Pool transaction spending `outpoint`, if any.
    pub fn spender(&self, outpoint: &OutPoint) -> Option<Txid> {
        self.spends.get(outpoint).copied()
    }

    fn reject(&mut self, wtxid: Wtxid) {
        if self.rejected.insert(wtxid) {
            self.rejected_order.push_back(wtxid);
            while self.rejected_order.len() > self.policy.rejected_cache {
                if let Some(old) = self.rejected_order.pop_front() {
                    self.rejected.remove(&old);
                }
            }
        }
    }
}

fn order_key(fee: u64, vsize: usize, txid: Txid) -> OrderKey {
    (fee.saturating_mul(1000) / vsize.max(1) as u64, txid)
}

/// Gossip payload: `version (1) ‖ count (u32 LE) ‖ (len (u32 LE) ‖ Elements tx)*`.
pub mod wire {
    use elements::{encode, Transaction};

    pub const VERSION: u8 = 1;
    /// Flush a batch at this size.
    pub const MAX_BATCH_BYTES: usize = 64 * 1024;

    pub fn encode_batch(txs: &[Transaction]) -> Vec<u8> {
        let mut out = vec![VERSION];
        out.extend_from_slice(&(txs.len() as u32).to_le_bytes());
        for tx in txs {
            let bytes = encode::serialize(tx);
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(&bytes);
        }
        out
    }

    /// Decodes a batch; `None` if malformed.
    pub fn decode_batch(bytes: &[u8]) -> Option<Vec<Transaction>> {
        if *bytes.first()? != VERSION {
            return None;
        }
        let mut pos = 1;
        let mut take = |n: usize| -> Option<&[u8]> {
            let s = bytes.get(pos..pos + n)?;
            pos += n;
            Some(s)
        };
        let count = u32::from_le_bytes(take(4)?.try_into().ok()?) as usize;
        let mut txs = Vec::with_capacity(count.min(10_000));
        for _ in 0..count {
            let len = u32::from_le_bytes(take(4)?.try_into().ok()?) as usize;
            txs.push(encode::deserialize(take(len)?).ok()?);
        }
        (pos == bytes.len()).then_some(txs)
    }
}
