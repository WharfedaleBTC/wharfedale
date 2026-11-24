//! Elements-subset consensus rules: stateless, contextual, amount and spend checks.
//!
//! No Malachite dependency. Spec: `docs/src/ledger/`.
//!
//! Covers explicit and confidential values and assets, issuance and reissuance, taproot
//! key-path and Simplicity (leaf `0xbe`) spends, locks, coinbase and fees. Simplicity programs
//! run in the C library through `wharfedale-ledger-simplicity-sys`.
#![forbid(unsafe_code)]

mod amounts;
mod block;
pub mod cache;
pub mod consts;
mod context;
pub mod error;
pub mod memory;
mod spend;
mod stateless;
pub mod types;

pub use block::connect_block;
pub use cache::VerifyCache;
pub use error::{BlockError, ProofLoc, TxError};
pub use memory::MemoryLedger;
pub use stateless::{coinbase_height_prefix, output_kind, OutputKind};
pub use types::{BlockDiff, ChainCtx, FeeMap, LedgerView, UtxoEntry};

use elements::Transaction;

/// Context-free checks of a non-coinbase transaction (pipeline stage 1). Returns its fees
/// (explicit fee outputs per asset), so policy can price it before full verification.
pub fn check_tx_stateless(tx: &Transaction) -> Result<FeeMap, TxError> {
    stateless::check(tx)
}

/// Full validation of a non-coinbase transaction against `view` for the block described by
/// `ctx`. Returns the transaction's fee map. Pipeline: `docs/src/ledger/validation.md`.
pub fn check_tx<V: LedgerView>(
    view: &V,
    tx: &Transaction,
    ctx: &ChainCtx,
    cache: &VerifyCache,
) -> Result<FeeMap, TxError> {
    let fees = stateless::check(tx)?;
    let spent = context::resolve_inputs(view, tx, ctx)?;
    context::check_final(tx, ctx)?;
    context::check_sequence_locks(tx, &spent, view, ctx)?;
    let wtxid = tx.wtxid();
    if !cache.contains(wtxid) {
        amounts::verify(tx, &spent)?;
        spend::verify(tx, &spent, ctx)?;
        cache.insert(wtxid);
    }
    Ok(fees)
}
