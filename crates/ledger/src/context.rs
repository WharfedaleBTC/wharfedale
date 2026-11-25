//! Stages 2–3: input resolution, coinbase maturity, absolute and relative locks.
//!
//! Mirrors Elements/Bitcoin `Consensus::CheckTxInputs` (maturity), `IsFinalTx` and
//! `CalculateSequenceLocks` / `EvaluateSequenceLocks` (BIP68), except that time locks compare
//! against block timestamps, not the median time past (BIP113): blocks are made only when there
//! is work, so a median of past blocks would stall while the chain is idle. A transaction whose
//! time lock has passed by the local clock is admitted, and its block carries a timestamp at
//! least that late.

use elements::Transaction;

use crate::consts::*;
use crate::error::TxError;
use crate::types::{ChainCtx, LedgerView, UtxoEntry};

/// Looks up every spent output and checks coinbase maturity.
pub(crate) fn resolve_inputs<V: LedgerView>(
    view: &V,
    tx: &Transaction,
    ctx: &ChainCtx,
) -> Result<Vec<UtxoEntry>, TxError> {
    tx.input
        .iter()
        .enumerate()
        .map(|(i, input)| {
            let entry = view
                .utxo(&input.previous_output)
                .ok_or(TxError::MissingInput(i, input.previous_output))?;
            if entry.is_coinbase && ctx.height < entry.height + ctx.coinbase_maturity {
                return Err(TxError::ImmatureCoinbase(i));
            }
            Ok(entry)
        })
        .collect()
}

/// nLockTime (`IsFinalTx`), compared against the block height or the block's timestamp.
pub(crate) fn check_final(tx: &Transaction, ctx: &ChainCtx) -> Result<(), TxError> {
    let lock_time = tx.lock_time.to_consensus_u32();
    if lock_time == 0 {
        return Ok(());
    }
    let threshold = if lock_time < LOCKTIME_THRESHOLD { ctx.height } else { ctx.time };
    if u64::from(lock_time) < threshold {
        return Ok(());
    }
    if tx.input.iter().all(|i| i.sequence.0 == SEQUENCE_FINAL) {
        return Ok(());
    }
    Err(TxError::NotFinal)
}

/// BIP68 relative locks. Every transaction is version 2, so they always apply.
pub(crate) fn check_sequence_locks<V: LedgerView>(
    tx: &Transaction,
    spent: &[UtxoEntry],
    view: &V,
    ctx: &ChainCtx,
) -> Result<(), TxError> {
    let mut min_height: i64 = -1;
    let mut min_time: i64 = -1;
    for (input, entry) in tx.input.iter().zip(spent) {
        let seq = input.sequence.0;
        if seq & SEQUENCE_LOCKTIME_DISABLE_FLAG != 0 {
            continue;
        }
        let coin_height = entry.height as i64;
        let units = i64::from(seq & SEQUENCE_LOCKTIME_MASK);
        if seq & SEQUENCE_LOCKTIME_TYPE_FLAG != 0 {
            let coin_time = time_at(view, ctx, entry.height) as i64;
            min_time = min_time.max(coin_time + (units << SEQUENCE_LOCKTIME_GRANULARITY) - 1);
        } else {
            min_height = min_height.max(coin_height + units - 1);
        }
    }
    if min_height >= ctx.height as i64 || min_time >= ctx.time as i64 {
        return Err(TxError::SequenceLocked);
    }
    Ok(())
}

/// Timestamp of the block that confirmed a coin: committed, or the block being validated
/// (a coin created earlier in the same block, or unconfirmed in the mempool).
fn time_at<V: LedgerView>(view: &V, ctx: &ChainCtx, height: u64) -> u64 {
    if height >= ctx.height {
        ctx.time
    } else {
        view.block_time(height)
    }
}
