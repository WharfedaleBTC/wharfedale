//! Block = header + Elements transactions.
//!
//! Encoding: `header (136 B) ‖ count: u32 LE ‖ (len: u32 LE ‖ elements consensus tx bytes)*`.

use elements::{encode, BlockHash, Transaction};

use crate::header::{Header, HEADER_SIZE};
use crate::merkle;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub header: Header,
    /// A coinbase, when the block has one, is `txs[0]`. It exists only to pay the block's fees.
    pub txs: Vec<Transaction>,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum BlockDecodeError {
    #[error("truncated block encoding")]
    Truncated,
    #[error("invalid header")]
    Header,
    #[error("invalid transaction {0}")]
    Transaction(usize),
    #[error("trailing bytes after the last transaction")]
    TrailingBytes,
}

impl Block {
    pub fn hash(&self) -> BlockHash {
        self.header.hash()
    }

    pub fn height(&self) -> u64 {
        self.header.height
    }

    /// Recomputes both merkle roots and compares them with the header.
    /// The header commits to exactly these transactions: both roots match and the txid tree
    /// is not mutated (no duplicated subtree yielding the same root).
    pub fn roots_match(&self) -> bool {
        !merkle::tx_tree_mutated(&self.txs)
            && self.header.tx_merkle_root == merkle::tx_root(&self.txs)
            && self.header.witness_merkle_root == merkle::witness_root(&self.txs)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_SIZE + 4 + self.txs.len() * 256);
        out.extend_from_slice(&self.header.encode());
        out.extend_from_slice(&(self.txs.len() as u32).to_le_bytes());
        for tx in &self.txs {
            let bytes = encode::serialize(tx);
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(&bytes);
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BlockDecodeError> {
        let header_bytes = bytes.get(..HEADER_SIZE).ok_or(BlockDecodeError::Truncated)?;
        let header = Header::decode(header_bytes).ok_or(BlockDecodeError::Header)?;
        let mut pos = HEADER_SIZE;
        let count = read_u32(bytes, &mut pos)? as usize;
        let mut txs = Vec::with_capacity(count.min(100_000));
        for i in 0..count {
            let len = read_u32(bytes, &mut pos)? as usize;
            let raw = bytes.get(pos..pos + len).ok_or(BlockDecodeError::Truncated)?;
            txs.push(encode::deserialize(raw).map_err(|_| BlockDecodeError::Transaction(i))?);
            pos += len;
        }
        if pos != bytes.len() {
            return Err(BlockDecodeError::TrailingBytes);
        }
        Ok(Self { header, txs })
    }
}

fn read_u32(bytes: &[u8], pos: &mut usize) -> Result<u32, BlockDecodeError> {
    let raw = bytes.get(*pos..*pos + 4).ok_or(BlockDecodeError::Truncated)?;
    *pos += 4;
    Ok(u32::from_le_bytes(raw.try_into().expect("4 bytes")))
}
