//! Block header and its fixed 136-byte encoding.

use elements::BlockHash;

use crate::{sha256d, Hash32};

/// Encoded header size in bytes.
pub const HEADER_SIZE: usize = 4 + 8 + 32 + 8 + 20 + 32 + 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub version: u32,
    pub height: u64,
    /// Hash of the parent header. In genesis: the hash of the genesis parameters.
    pub prev_block_hash: Hash32,
    /// Unix seconds.
    pub timestamp: u64,
    /// Consensus address of the proposer (zero in genesis).
    pub proposer: [u8; 20],
    /// Merkle root over Elements txids.
    pub tx_merkle_root: Hash32,
    /// Merkle root over wtxids; the coinbase contributes zero.
    pub witness_merkle_root: Hash32,
}

impl Header {
    pub const VERSION: u32 = 1;

    /// Little-endian integers, fields in declaration order.
    pub fn encode(&self) -> [u8; HEADER_SIZE] {
        let mut out = [0u8; HEADER_SIZE];
        let mut w = Writer { buf: &mut out, pos: 0 };
        w.put(&self.version.to_le_bytes());
        w.put(&self.height.to_le_bytes());
        w.put(&self.prev_block_hash);
        w.put(&self.timestamp.to_le_bytes());
        w.put(&self.proposer);
        w.put(&self.tx_merkle_root);
        w.put(&self.witness_merkle_root);
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != HEADER_SIZE {
            return None;
        }
        let mut r = Reader { buf: bytes, pos: 0 };
        Some(Self {
            version: u32::from_le_bytes(r.take()),
            height: u64::from_le_bytes(r.take()),
            prev_block_hash: r.take(),
            timestamp: u64::from_le_bytes(r.take()),
            proposer: r.take(),
            tx_merkle_root: r.take(),
            witness_merkle_root: r.take(),
        })
    }

    /// Block hash = SHA-256d(encoded header).
    pub fn hash(&self) -> BlockHash {
        BlockHash::from_byte_array(sha256d(&self.encode()))
    }
}

struct Writer<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl Writer<'_> {
    fn put(&mut self, bytes: &[u8]) {
        self.buf[self.pos..self.pos + bytes.len()].copy_from_slice(bytes);
        self.pos += bytes.len();
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> [u8; N] {
        let out = self.buf[self.pos..self.pos + N].try_into().expect("length checked");
        self.pos += N;
        out
    }
}
