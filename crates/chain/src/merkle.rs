//! Bitcoin-style merkle roots over txids and wtxids.

use elements::Transaction;

use crate::{sha256d, Hash32};

/// Merkle root of `leaves`: pairwise SHA-256d, duplicating the last node of an odd level.
/// The root of an empty list is zero.
pub fn root(level: Vec<Hash32>) -> Hash32 {
    root_checked(level).0
}

/// Root and whether the tree is *mutated*: some level has two equal siblings. Duplicating the
/// last node makes `[a, b, c]` and `[a, b, c, c]` share a root (CVE-2012-2459); Bitcoin's
/// `ComputeMerkleRoot` flags it the same way.
pub fn root_checked(mut level: Vec<Hash32>) -> (Hash32, bool) {
    if level.is_empty() {
        return ([0; 32], false);
    }
    let mut mutated = false;
    while level.len() > 1 {
        mutated |= level.as_chunks::<2>().0.iter().any(|[l, r]| l == r);
        if level.len() % 2 == 1 {
            level.push(*level.last().expect("non-empty"));
        }
        level = level
            .chunks(2)
            .map(|pair| {
                let mut buf = [0u8; 64];
                buf[..32].copy_from_slice(&pair[0]);
                buf[32..].copy_from_slice(&pair[1]);
                sha256d(&buf)
            })
            .collect();
    }
    (level[0], mutated)
}

/// Root over the transactions' txids.
pub fn tx_root(txs: &[Transaction]) -> Hash32 {
    root(txs.iter().map(|tx| tx.txid().to_byte_array()).collect())
}

/// Whether the txid tree is mutated (see [`root_checked`]): such a transaction list is never
/// a valid block body.
pub fn tx_tree_mutated(txs: &[Transaction]) -> bool {
    root_checked(txs.iter().map(|tx| tx.txid().to_byte_array()).collect()).1
}

/// Root over the transactions' wtxids; a leading coinbase contributes zero (BIP141).
pub fn witness_root(txs: &[Transaction]) -> Hash32 {
    root(
        txs.iter()
            .enumerate()
            .map(|(i, tx)| if i == 0 && tx.is_coinbase() { [0; 32] } else { tx.wtxid().to_byte_array() })
            .collect(),
    )
}
