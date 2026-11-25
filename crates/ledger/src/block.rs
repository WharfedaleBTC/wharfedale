//! Block-level validation: coinbase, per-transaction checks in order, fee claim, UTXO diff.
//!
//! Mirrors Elements `ConnectBlock` (transaction loop) and `VerifyCoinbaseAmount`
//! (`src/confidential_validation.cpp:425`) with zero subsidy. Deviation: the coinbase is
//! optional. It exists only to pay the block's fees to the proposer, so a block whose
//! transactions pay none needs no coinbase; fees in a block without one are burned.

use std::collections::{BTreeMap, HashMap, HashSet};

use elements::{confidential, AssetId, OutPoint, Transaction};

use crate::consts::{MAX_BLOCK_WEIGHT, MAX_MONEY};
use crate::error::{BlockError, TxError};
use crate::stateless::{check_coinbase, output_kind, OutputKind};
use crate::types::{BlockDiff, ChainCtx, FeeMap, LedgerView, UtxoEntry};
use crate::{check_tx, context};

/// Validates `txs` (an optional coinbase first, then the rest) on top of `view` and returns
/// the state change. Pure: nothing is written; the caller applies the [`BlockDiff`].
///
/// Any transaction list is structurally acceptable, the empty one included (blocks made before
/// proposers stopped building empty ones). Not proposing empty blocks is the proposer's policy.
pub fn connect_block<V: LedgerView>(
    view: &V,
    txs: &[Transaction],
    ctx: &ChainCtx,
) -> Result<BlockDiff, BlockError> {
    // WHARFEDALE: the coinbase is optional (Elements requires one); it only pays fees.
    let (coinbase, rest) = match txs.split_first() {
        Some((first, rest)) if first.is_coinbase() => (Some(first), rest),
        _ => (None, txs),
    };
    let offset = usize::from(coinbase.is_some());

    let mut weight = 0;
    let mut overlay = Overlay::new(view);
    if let Some(coinbase) = coinbase {
        check_coinbase(coinbase, ctx.height)?;
        context::check_final(coinbase, ctx)
            .map_err(|_| BlockError::Coinbase("lock_time not reached"))?;
        weight = coinbase.weight();
        overlay.apply(coinbase, ctx.height, true)?;
    }

    let mut fees = FeeMap::new();
    for (k, tx) in rest.iter().enumerate() {
        let index = k + offset;
        if tx.is_coinbase() {
            return Err(BlockError::ExtraCoinbase(index));
        }
        weight += tx.weight();
        if weight > MAX_BLOCK_WEIGHT {
            return Err(BlockError::TooHeavy(weight));
        }
        let tx_fees = check_tx(&overlay, tx, ctx)
            .map_err(|error| BlockError::Tx { index, txid: tx.txid(), error })?;
        for (asset, fee) in tx_fees {
            let total = fees.entry(asset).or_insert(0);
            *total += fee; // both <= MAX_MONEY
            if *total > MAX_MONEY {
                return Err(BlockError::Tx {
                    index,
                    txid: tx.txid(),
                    error: TxError::FeeOutOfRange(asset),
                });
            }
        }
        overlay.apply(tx, ctx.height, false)?;
    }

    // Coinbase may claim at most the collected fees, per asset (subsidy 0).
    let mut claimed: BTreeMap<AssetId, u128> = BTreeMap::new();
    for out in coinbase.iter().flat_map(|c| &c.output) {
        if let (Some(asset), confidential::Value::Explicit(v)) = (out.asset.explicit(), out.value) {
            *claimed.entry(asset).or_insert(0) += u128::from(v);
        }
    }
    for (asset, amount) in claimed {
        if amount > u128::from(fees.get(&asset).copied().unwrap_or(0)) {
            return Err(BlockError::CoinbaseOverclaim(asset));
        }
    }

    Ok(overlay.into_diff(fees))
}

/// The base view plus the effects of the block's transactions applied so far.
struct Overlay<'a, V> {
    base: &'a V,
    created: HashMap<OutPoint, UtxoEntry>,
    created_order: Vec<OutPoint>,
    spent_base: Vec<(OutPoint, UtxoEntry)>,
    spent_set: HashSet<OutPoint>,
}

impl<'a, V: LedgerView> Overlay<'a, V> {
    fn new(base: &'a V) -> Self {
        Self {
            base,
            created: HashMap::new(),
            created_order: Vec::new(),
            spent_base: Vec::new(),
            spent_set: HashSet::new(),
        }
    }

    /// Spends the (already validated) inputs and adds the spendable outputs of `tx`.
    fn apply(&mut self, tx: &Transaction, height: u64, is_coinbase: bool) -> Result<(), BlockError> {
        if !is_coinbase {
            for input in &tx.input {
                let op = input.previous_output;
                if self.created.remove(&op).is_none() {
                    let entry = self.base.utxo(&op).expect("input validated against this view");
                    self.spent_set.insert(op);
                    self.spent_base.push((op, entry));
                }
            }
        }
        let txid = tx.txid();
        for (vout, out) in tx.output.iter().enumerate() {
            // Fee and OP_RETURN outputs never enter the UTXO set.
            if output_kind(out) != Some(OutputKind::Taproot) {
                continue;
            }
            let op = OutPoint::new(txid, vout as u32);
            // BIP30: never overwrite an unspent output.
            if self.utxo(&op).is_some() {
                return Err(BlockError::OutputExists(op));
            }
            self.created.insert(op, UtxoEntry::new(out.clone(), height, is_coinbase));
            self.created_order.push(op);
        }
        Ok(())
    }

    fn into_diff(mut self, fees: FeeMap) -> BlockDiff {
        let created = self
            .created_order
            .iter()
            .filter_map(|op| self.created.remove(op).map(|e| (*op, e)))
            .collect();
        BlockDiff { spent: self.spent_base, created, fees }
    }
}

impl<V: LedgerView> LedgerView for Overlay<'_, V> {
    fn utxo(&self, outpoint: &OutPoint) -> Option<UtxoEntry> {
        if let Some(entry) = self.created.get(outpoint) {
            return Some(entry.clone());
        }
        if self.spent_set.contains(outpoint) {
            return None;
        }
        self.base.utxo(outpoint)
    }

    fn median_time_past(&self, height: u64) -> u64 {
        self.base.median_time_past(height)
    }
}
