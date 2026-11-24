//! Record encodings.
//!
//! - outpoint: `txid (32) ‖ vout (u32 LE)`
//! - UTXO entry: `height (u64 LE) ‖ is_coinbase (u8) ‖ TxOut consensus bytes (no witness)`
//! - undo: `count (u32 LE) ‖ (outpoint ‖ len (u32 LE) ‖ entry)*`

use elements::{encode, OutPoint, TxOut};
use wharfedale_ledger::UtxoEntry;

pub fn encode_outpoint(op: &OutPoint) -> [u8; 36] {
    let mut k = [0u8; 36];
    k[..32].copy_from_slice(&op.txid.to_byte_array());
    k[32..].copy_from_slice(&op.vout.to_le_bytes());
    k
}

pub fn decode_outpoint(bytes: &[u8]) -> Option<OutPoint> {
    let txid = elements::Txid::from_byte_array(bytes.get(..32)?.try_into().ok()?);
    let vout = u32::from_le_bytes(bytes.get(32..36)?.try_into().ok()?);
    Some(OutPoint::new(txid, vout))
}

pub fn encode_entry(entry: &UtxoEntry) -> Vec<u8> {
    let mut out = entry.height.to_le_bytes().to_vec();
    out.push(u8::from(entry.is_coinbase));
    out.extend_from_slice(&encode::serialize(&entry.txout));
    out
}

pub fn decode_entry(bytes: &[u8]) -> Option<UtxoEntry> {
    let height = u64::from_le_bytes(bytes.get(..8)?.try_into().ok()?);
    let is_coinbase = *bytes.get(8)? != 0;
    let txout: TxOut = encode::deserialize(bytes.get(9..)?).ok()?;
    Some(UtxoEntry::new(txout, height, is_coinbase))
}

pub(crate) fn encode_undo(spent: &[(OutPoint, UtxoEntry)]) -> Vec<u8> {
    let mut out = (spent.len() as u32).to_le_bytes().to_vec();
    for (op, entry) in spent {
        out.extend_from_slice(&encode_outpoint(op));
        let e = encode_entry(entry);
        out.extend_from_slice(&(e.len() as u32).to_le_bytes());
        out.extend_from_slice(&e);
    }
    out
}
