//! redb storage: core tables, in-flight proposals, atomic commit.
//!
//! Spec: `docs/src/storage.md`. Query indexes and the indexer arrive with RPC (M3).
#![forbid(unsafe_code)]

mod codec;
mod index;

pub use index::{
    asset_metadata, parse_asset_metadata, script_hash, wfa1_payload, AssetMetadata, AssetRecord, WFA1_MAX_METADATA, WFA1_TAG,
};

use std::path::Path;

use elements::{BlockHash, OutPoint, Script, Transaction, Txid};
use redb::{Database, ReadableTable, TableDefinition};
use wharfedale_chain::{genesis_diff, median_time_past, Block, Genesis, Header, MTP_WINDOW};
use wharfedale_ledger::{BlockDiff, LedgerView, UtxoEntry};

pub use codec::{decode_entry, decode_outpoint, encode_entry, encode_outpoint};

// Core
pub(crate) const META: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");
const HEADERS: TableDefinition<u64, &[u8]> = TableDefinition::new("headers");
const BODIES: TableDefinition<u64, &[u8]> = TableDefinition::new("bodies");
const BLOCK_INDEX: TableDefinition<&[u8], u64> = TableDefinition::new("block_index");
const CERTS: TableDefinition<u64, &[u8]> = TableDefinition::new("certs");
const UTXOS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("utxos");
const UNDO: TableDefinition<u64, &[u8]> = TableDefinition::new("undo");
// In-flight; keys start with the height (big-endian) so committed heights prune by range.
const UNDECIDED: TableDefinition<&[u8], &[u8]> = TableDefinition::new("undecided");
const BUILT: TableDefinition<&[u8], &[u8]> = TableDefinition::new("built_proposals");
const PARTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("undecided_parts");
// Query indexes, maintained in the commit transaction.
/// SHA-256(scriptPubKey) ‖ outpoint → ()
const SPK_UTXOS: TableDefinition<&[u8], ()> = TableDefinition::new("spk_utxos");
/// txid → height (u64 LE) ‖ position in block (u32 LE)
const TX_INDEX: TableDefinition<&[u8], &[u8]> = TableDefinition::new("tx_index");

const SCHEMA_VERSION: &[u8] = b"2";
/// Blocks of undo data kept for operator rollback tools.
pub const UNDO_RETENTION: u64 = 1_000;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database: {0}")]
    Db(String),
    #[error("store is not initialised")]
    NotInitialised,
    #[error("store belongs to another chain (chain id or genesis hash differ)")]
    WrongChain,
    #[error("schema version mismatch")]
    Schema,
    #[error("block {got} does not extend the tip {tip}")]
    NotNext { got: u64, tip: u64 },
    #[error("corrupt record: {0}")]
    Corrupt(&'static str),
}

pub type Result<T> = std::result::Result<T, StoreError>;

pub(crate) fn dberr<E: Into<redb::Error>>(e: E) -> StoreError {
    StoreError::Db(e.into().to_string())
}

pub struct Store {
    pub(crate) db: Database,
}

impl Store {
    /// Opens (or creates) the database and all tables.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db = Database::create(path).map_err(dberr)?;
        let txn = db.begin_write().map_err(dberr)?;
        {
            txn.open_table(META).map_err(dberr)?;
            txn.open_table(HEADERS).map_err(dberr)?;
            txn.open_table(BODIES).map_err(dberr)?;
            txn.open_table(BLOCK_INDEX).map_err(dberr)?;
            txn.open_table(CERTS).map_err(dberr)?;
            txn.open_table(UTXOS).map_err(dberr)?;
            txn.open_table(UNDO).map_err(dberr)?;
            txn.open_table(UNDECIDED).map_err(dberr)?;
            txn.open_table(BUILT).map_err(dberr)?;
            txn.open_table(PARTS).map_err(dberr)?;
            txn.open_table(SPK_UTXOS).map_err(dberr)?;
            txn.open_table(TX_INDEX).map_err(dberr)?;
            txn.open_table(index::SPK_HISTORY).map_err(dberr)?;
            txn.open_table(index::SPENT_BY).map_err(dberr)?;
            txn.open_table(index::ASSETS).map_err(dberr)?;
            txn.open_table(index::ASSET_HISTORY).map_err(dberr)?;
        }
        txn.commit().map_err(dberr)?;
        let store = Self { db };
        store.check_index_version()?;
        Ok(store)
    }

    /// Initialises an empty store from genesis, or checks that an existing store matches it.
    pub fn init_or_check(&self, genesis: &Genesis) -> Result<()> {
        let block = genesis.block().map_err(|_| StoreError::Corrupt("genesis"))?;
        let hash = block.hash();
        match self.meta("genesis_hash")? {
            Some(stored) => {
                if self.meta("schema_version")?.as_deref() != Some(SCHEMA_VERSION) {
                    return Err(StoreError::Schema);
                }
                if stored != hash.to_byte_array()
                    || self.meta("chain_id")?.as_deref() != Some(genesis.chain_id.as_bytes())
                {
                    return Err(StoreError::WrongChain);
                }
                Ok(())
            }
            None => {
                let txn = self.db.begin_write().map_err(dberr)?;
                {
                    let mut meta = txn.open_table(META).map_err(dberr)?;
                    meta.insert("schema_version", SCHEMA_VERSION).map_err(dberr)?;
                    meta.insert("chain_id", genesis.chain_id.as_bytes()).map_err(dberr)?;
                    meta.insert("genesis_hash", hash.to_byte_array().as_slice()).map_err(dberr)?;
                }
                write_block(&txn, &block, &[], &genesis_diff(&block))?;
                txn.commit().map_err(dberr)?;
                Ok(())
            }
        }
    }

    pub fn genesis_hash(&self) -> Result<BlockHash> {
        let raw = self.meta("genesis_hash")?.ok_or(StoreError::NotInitialised)?;
        Ok(BlockHash::from_byte_array(raw.try_into().map_err(|_| StoreError::Corrupt("genesis_hash"))?))
    }

    /// `(height, hash)` of the last committed block.
    pub fn tip(&self) -> Result<(u64, BlockHash)> {
        let height = self.meta("tip_height")?.ok_or(StoreError::NotInitialised)?;
        let hash = self.meta("tip_hash")?.ok_or(StoreError::NotInitialised)?;
        Ok((
            u64::from_le_bytes(height.try_into().map_err(|_| StoreError::Corrupt("tip_height"))?),
            BlockHash::from_byte_array(hash.try_into().map_err(|_| StoreError::Corrupt("tip_hash"))?),
        ))
    }

    pub fn header(&self, height: u64) -> Result<Option<Header>> {
        self.get_u64(HEADERS, height)?
            .map(|b| Header::decode(&b).ok_or(StoreError::Corrupt("header")))
            .transpose()
    }

    pub fn block_bytes(&self, height: u64) -> Result<Option<Vec<u8>>> {
        self.get_u64(BODIES, height)
    }

    pub fn block(&self, height: u64) -> Result<Option<Block>> {
        self.block_bytes(height)?
            .map(|b| Block::decode(&b).map_err(|_| StoreError::Corrupt("block")))
            .transpose()
    }

    pub fn height_of(&self, hash: &BlockHash) -> Result<Option<u64>> {
        let txn = self.db.begin_read().map_err(dberr)?;
        let table = txn.open_table(BLOCK_INDEX).map_err(dberr)?;
        Ok(table.get(hash.to_byte_array().as_slice()).map_err(dberr)?.map(|v| v.value()))
    }

    /// Encoded commit certificate of `height`.
    pub fn cert(&self, height: u64) -> Result<Option<Vec<u8>>> {
        self.get_u64(CERTS, height)
    }

    /// Lowest height with a stored body.
    pub fn min_body_height(&self) -> Result<u64> {
        let txn = self.db.begin_read().map_err(dberr)?;
        let table = txn.open_table(BODIES).map_err(dberr)?;
        let first = table.first().map_err(dberr)?.map(|(k, _)| k.value());
        Ok(first.unwrap_or(0))
    }

    /// Median time past of the committed block at `height` (Esplora `mediantime`; consensus
    /// uses block timestamps).
    pub fn mtp(&self, height: u64) -> Result<u64> {
        let txn = self.db.begin_read().map_err(dberr)?;
        let table = txn.open_table(HEADERS).map_err(dberr)?;
        let from = height.saturating_sub(MTP_WINDOW as u64 - 1);
        let mut times = Vec::with_capacity(MTP_WINDOW);
        for item in table.range(from..=height).map_err(dberr)? {
            let (_, v) = item.map_err(dberr)?;
            times.push(Header::decode(v.value()).ok_or(StoreError::Corrupt("header"))?.timestamp);
        }
        Ok(median_time_past(&times))
    }

    /// Atomically commits a decided block: header, body, index, certificate, UTXO diff, undo,
    /// tip, and prunes in-flight records at or below its height. fsynced on return.
    pub fn commit(&self, block: &Block, cert: &[u8], diff: &BlockDiff) -> Result<()> {
        let (tip, tip_hash) = self.tip()?;
        if block.height() != tip + 1 || block.header.prev_block_hash != tip_hash.to_byte_array() {
            return Err(StoreError::NotNext { got: block.height(), tip });
        }
        let txn = self.db.begin_write().map_err(dberr)?;
        write_block(&txn, block, cert, diff)?;
        {
            let mut undo = txn.open_table(UNDO).map_err(dberr)?;
            undo.insert(block.height(), codec::encode_undo(&diff.spent).as_slice()).map_err(dberr)?;
            if let Some(old) = block.height().checked_sub(UNDO_RETENTION) {
                undo.remove(old).map_err(dberr)?;
            }
            let upper = (block.height() + 1).to_be_bytes();
            for table in [UNDECIDED, BUILT, PARTS] {
                let mut t = txn.open_table(table).map_err(dberr)?;
                t.retain_in::<&[u8], _>(..upper.as_slice(), |_, _| false).map_err(dberr)?;
            }
        }
        txn.commit().map_err(dberr)?;
        Ok(())
    }

    /// Replaces the certificate of a committed height (the extended one from `Finalized`).
    pub fn put_cert(&self, height: u64, cert: &[u8]) -> Result<()> {
        let txn = self.db.begin_write().map_err(dberr)?;
        txn.open_table(CERTS).map_err(dberr)?.insert(height, cert).map_err(dberr)?;
        txn.commit().map_err(dberr)
    }

    // ---- query indexes ----

    /// The unspent output at `outpoint`.
    pub fn utxo_entry(&self, outpoint: &OutPoint) -> Result<Option<UtxoEntry>> {
        self.get_raw(UTXOS, &encode_outpoint(outpoint))?
            .map(|b| decode_entry(&b).ok_or(StoreError::Corrupt("utxo")))
            .transpose()
    }

    /// Unspent outputs paying to `script_pubkey`.
    pub fn utxos_by_script(&self, script_pubkey: &Script) -> Result<Vec<(OutPoint, UtxoEntry)>> {
        let prefix = script_hash(script_pubkey);
        let txn = self.db.begin_read().map_err(dberr)?;
        let index = txn.open_table(SPK_UTXOS).map_err(dberr)?;
        let utxos = txn.open_table(UTXOS).map_err(dberr)?;
        let mut out = Vec::new();
        for item in index.range::<&[u8]>(prefix.as_slice()..).map_err(dberr)? {
            let (k, _) = item.map_err(dberr)?;
            let k = k.value();
            if !k.starts_with(&prefix) {
                break;
            }
            let op_bytes = &k[32..];
            let op = codec::decode_outpoint(op_bytes).ok_or(StoreError::Corrupt("spk index"))?;
            let raw = utxos.get(op_bytes).map_err(dberr)?.ok_or(StoreError::Corrupt("spk index"))?;
            out.push((op, decode_entry(raw.value()).ok_or(StoreError::Corrupt("utxo"))?));
        }
        Ok(out)
    }

    /// A committed transaction and the height of its block.
    pub fn transaction(&self, txid: &Txid) -> Result<Option<(Transaction, u64)>> {
        let Some(loc) = self.get_raw(TX_INDEX, &txid.to_byte_array())? else { return Ok(None) };
        let height = u64::from_le_bytes(loc.get(..8).and_then(|b| b.try_into().ok()).ok_or(StoreError::Corrupt("tx_index"))?);
        let pos = u32::from_le_bytes(loc.get(8..12).and_then(|b| b.try_into().ok()).ok_or(StoreError::Corrupt("tx_index"))?) as usize;
        // Decode just this transaction, in place: RPC and the indexer call this per input, and
        // a block can be megabytes.
        let txn = self.db.begin_read().map_err(dberr)?;
        let bodies = txn.open_table(BODIES).map_err(dberr)?;
        let body = bodies.get(height).map_err(dberr)?.ok_or(StoreError::Corrupt("tx_index points to missing block"))?;
        let tx = Block::decode_tx_at(body.value(), pos)
            .map_err(|_| StoreError::Corrupt("block"))?
            .ok_or(StoreError::Corrupt("tx_index position"))?;
        Ok(Some((tx, height)))
    }

    // ---- in-flight proposals ----

    /// Stores a proposal record (node-defined encoding) for (height, round, block hash).
    pub fn put_undecided(&self, height: u64, round: u32, hash: &BlockHash, record: &[u8]) -> Result<()> {
        self.put_raw(UNDECIDED, &key_hrh(height, round, hash), record)
    }

    /// All proposal records for (height, round).
    pub fn undecided_in_round(&self, height: u64, round: u32) -> Result<Vec<Vec<u8>>> {
        let mut prefix = height.to_be_bytes().to_vec();
        prefix.extend_from_slice(&round.to_be_bytes());
        self.scan_prefix(UNDECIDED, &prefix)
    }

    /// Any proposal record for `hash` at `height`, in any round.
    pub fn undecided_by_hash(&self, height: u64, hash: &BlockHash) -> Result<Option<Vec<u8>>> {
        let txn = self.db.begin_read().map_err(dberr)?;
        let table = txn.open_table(UNDECIDED).map_err(dberr)?;
        let prefix = height.to_be_bytes();
        for item in table.range::<&[u8]>(prefix.as_slice()..).map_err(dberr)? {
            let (k, v) = item.map_err(dberr)?;
            let k = k.value();
            if !k.starts_with(&prefix) {
                break;
            }
            if k[12..] == hash.to_byte_array() {
                return Ok(Some(v.value().to_vec()));
            }
        }
        Ok(None)
    }

    /// Records the block hash we proposed at (height, round).
    pub fn put_built(&self, height: u64, round: u32, hash: &BlockHash) -> Result<()> {
        self.put_raw(BUILT, &key_hr(height, round), &hash.to_byte_array())
    }

    pub fn built(&self, height: u64, round: u32) -> Result<Option<BlockHash>> {
        Ok(self
            .get_raw(BUILT, &key_hr(height, round))?
            .and_then(|b| <[u8; 32]>::try_from(b).ok())
            .map(BlockHash::from_byte_array))
    }

    /// Stores the signed proposal parts of a block (for restreaming).
    pub fn put_parts(&self, height: u64, hash: &BlockHash, parts: &[u8]) -> Result<()> {
        self.put_raw(PARTS, &key_hh(height, hash), parts)
    }

    pub fn parts(&self, height: u64, hash: &BlockHash) -> Result<Option<Vec<u8>>> {
        self.get_raw(PARTS, &key_hh(height, hash))
    }

    // ---- helpers ----

    pub(crate) fn meta(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let txn = self.db.begin_read().map_err(dberr)?;
        let table = txn.open_table(META).map_err(dberr)?;
        Ok(table.get(key).map_err(dberr)?.map(|v| v.value().to_vec()))
    }

    fn get_u64(&self, def: TableDefinition<u64, &[u8]>, key: u64) -> Result<Option<Vec<u8>>> {
        let txn = self.db.begin_read().map_err(dberr)?;
        let table = txn.open_table(def).map_err(dberr)?;
        Ok(table.get(key).map_err(dberr)?.map(|v| v.value().to_vec()))
    }

    fn get_raw(&self, def: TableDefinition<&[u8], &[u8]>, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let txn = self.db.begin_read().map_err(dberr)?;
        let table = txn.open_table(def).map_err(dberr)?;
        Ok(table.get(key).map_err(dberr)?.map(|v| v.value().to_vec()))
    }

    fn put_raw(&self, def: TableDefinition<&[u8], &[u8]>, key: &[u8], value: &[u8]) -> Result<()> {
        let txn = self.db.begin_write().map_err(dberr)?;
        txn.open_table(def).map_err(dberr)?.insert(key, value).map_err(dberr)?;
        txn.commit().map_err(dberr)
    }

    fn scan_prefix(&self, def: TableDefinition<&[u8], &[u8]>, prefix: &[u8]) -> Result<Vec<Vec<u8>>> {
        let txn = self.db.begin_read().map_err(dberr)?;
        let table = txn.open_table(def).map_err(dberr)?;
        let mut out = Vec::new();
        for item in table.range::<&[u8]>(prefix..).map_err(dberr)? {
            let (k, v) = item.map_err(dberr)?;
            if !k.value().starts_with(prefix) {
                break;
            }
            out.push(v.value().to_vec());
        }
        Ok(out)
    }
}

/// Writes header, body, index, certificate, UTXO changes and the tip into `txn`.
fn write_block(txn: &redb::WriteTransaction, block: &Block, cert: &[u8], diff: &BlockDiff) -> Result<()> {
    let height = block.height();
    let hash = block.hash().to_byte_array();
    txn.open_table(HEADERS).map_err(dberr)?.insert(height, block.header.encode().as_slice()).map_err(dberr)?;
    txn.open_table(BODIES).map_err(dberr)?.insert(height, block.encode().as_slice()).map_err(dberr)?;
    txn.open_table(BLOCK_INDEX).map_err(dberr)?.insert(hash.as_slice(), height).map_err(dberr)?;
    txn.open_table(CERTS).map_err(dberr)?.insert(height, cert).map_err(dberr)?;
    {
        let mut utxos = txn.open_table(UTXOS).map_err(dberr)?;
        let mut spk = txn.open_table(SPK_UTXOS).map_err(dberr)?;
        for (op, entry) in &diff.spent {
            utxos.remove(encode_outpoint(op).as_slice()).map_err(dberr)?;
            spk.remove(spk_key(&entry.txout.script_pubkey, op).as_slice()).map_err(dberr)?;
        }
        for (op, entry) in &diff.created {
            utxos.insert(encode_outpoint(op).as_slice(), encode_entry(entry).as_slice()).map_err(dberr)?;
            spk.insert(spk_key(&entry.txout.script_pubkey, op).as_slice(), ()).map_err(dberr)?;
        }
        let mut index = txn.open_table(TX_INDEX).map_err(dberr)?;
        for (pos, tx) in block.txs.iter().enumerate() {
            let mut loc = height.to_le_bytes().to_vec();
            loc.extend_from_slice(&(pos as u32).to_le_bytes());
            index.insert(tx.txid().to_byte_array().as_slice(), loc.as_slice()).map_err(dberr)?;
        }
    }
    let mut meta = txn.open_table(META).map_err(dberr)?;
    meta.insert("tip_height", height.to_le_bytes().as_slice()).map_err(dberr)?;
    meta.insert("tip_hash", hash.as_slice()).map_err(dberr)?;
    Ok(())
}

fn spk_key(spk: &Script, op: &OutPoint) -> Vec<u8> {
    let mut k = script_hash(spk).to_vec();
    k.extend_from_slice(&encode_outpoint(op));
    k
}

fn key_hr(height: u64, round: u32) -> Vec<u8> {
    let mut k = height.to_be_bytes().to_vec();
    k.extend_from_slice(&round.to_be_bytes());
    k
}

fn key_hrh(height: u64, round: u32, hash: &BlockHash) -> Vec<u8> {
    let mut k = key_hr(height, round);
    k.extend_from_slice(&hash.to_byte_array());
    k
}

fn key_hh(height: u64, hash: &BlockHash) -> Vec<u8> {
    let mut k = height.to_be_bytes().to_vec();
    k.extend_from_slice(&hash.to_byte_array());
    k
}

impl LedgerView for Store {
    fn utxo(&self, outpoint: &OutPoint) -> Option<UtxoEntry> {
        let raw = self
            .get_raw(UTXOS, &encode_outpoint(outpoint))
            .expect("storage read failed");
        raw.map(|b| decode_entry(&b).expect("corrupt UTXO entry"))
    }

    fn block_time(&self, height: u64) -> u64 {
        self.header(height).expect("storage read failed").expect("committed header").timestamp
    }
}
