//! Ledger data types and the view the ledger reads state through.

use std::collections::BTreeMap;

use elements::{AssetId, BlockHash, OutPoint, TxOut, TxOutWitness};

/// An unspent output as stored in the UTXO set. Output witnesses (proofs) are never stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UtxoEntry {
    pub txout: TxOut,
    /// Height of the block that created the output.
    pub height: u64,
    pub is_coinbase: bool,
}

impl UtxoEntry {
    /// Builds an entry, stripping the output witness.
    pub fn new(mut txout: TxOut, height: u64, is_coinbase: bool) -> Self {
        txout.witness = TxOutWitness::empty();
        Self { txout, height, is_coinbase }
    }
}

/// Read access to committed state. Implemented by the store, by overlays and by tests.
pub trait LedgerView {
    /// The unspent output at `outpoint`, if any.
    fn utxo(&self, outpoint: &OutPoint) -> Option<UtxoEntry>;

    /// Timestamp of the committed block at `height`. Only called for heights below the block
    /// being validated.
    fn block_time(&self, height: u64) -> u64;
}

/// Context of the block a transaction is validated for.
#[derive(Clone, Debug)]
pub struct ChainCtx {
    /// Height of the block being validated (tip + 1 for mempool admission).
    pub height: u64,
    /// Timestamp of the block being validated; for mempool admission, the earliest timestamp a
    /// block built now could carry (`max(now, tip + 1)`). Time locks compare against it.
    pub time: u64,
    /// Genesis block hash, committed by every taproot sighash and by Simplicity.
    pub genesis_hash: BlockHash,
    /// Blocks a coinbase output must wait before it can be spent.
    pub coinbase_maturity: u64,
}

/// Total explicit fees per asset.
pub type FeeMap = BTreeMap<AssetId, u64>;

/// State change of one block, produced by [`crate::connect_block`] and applied by the store.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockDiff {
    /// Outputs that existed before the block and are spent by it, with their entries (undo data).
    pub spent: Vec<(OutPoint, UtxoEntry)>,
    /// Outputs created by the block and still unspent at its end, in creation order.
    pub created: Vec<(OutPoint, UtxoEntry)>,
    /// Fees collected by the block's transactions.
    pub fees: FeeMap,
}
