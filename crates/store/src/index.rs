//! Query indexer for the Esplora API: script history, spent-by, assets. Runs behind the tip
//! with its own cursor (`meta.indexed_height`); never blocks consensus; rebuildable from blocks.

use elements::confidential::Value;
use elements::hashes::sha256;
use elements::{AssetId, OutPoint, Script, Transaction, Txid};
use redb::{ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

use crate::codec::encode_outpoint;
use crate::{dberr, Result, Store, StoreError, META};

/// script_hash (32) ‖ height (u64 BE) ‖ position (u32 BE) ‖ txid (32) → ()
pub(crate) const SPK_HISTORY: TableDefinition<&[u8], ()> = TableDefinition::new("spk_history");
/// outpoint (36) → spending txid (32) ‖ vin (u32 LE)
pub(crate) const SPENT_BY: TableDefinition<&[u8], &[u8]> = TableDefinition::new("spent_by");
/// asset id (32) → JSON `AssetRecord`
pub(crate) const ASSETS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("assets");
/// asset id (32) ‖ height (u64 BE) ‖ position (u32 BE) ‖ txid (32) → (): the transactions that
/// issue, reissue or burn the asset (Esplora's asset history).
pub(crate) const ASSET_HISTORY: TableDefinition<&[u8], ()> = TableDefinition::new("asset_history");

/// Version of the query indexes. A store with another version drops them and rebuilds them
/// from its blocks ([`Store::check_index_version`]).
pub(crate) const INDEX_VERSION: &[u8] = b"2";

/// SHA-256 of a scriptPubKey (Esplora `scripthash`).
pub fn script_hash(spk: &Script) -> [u8; 32] {
    sha256::Hash::hash(spk.as_bytes()).to_byte_array()
}

/// What the indexer knows about an issued asset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetRecord {
    pub asset_id: String,
    pub reissuance_token: String,
    pub issuance_txid: String,
    pub issuance_vin: u32,
    pub issuance_prevout: (String, u32),
    pub contract_hash: String,
    /// Issuance entropy (hex); needed to reissue.
    pub entropy: String,
    pub height: u64,
    pub issuance_count: u64,
    /// Transactions in the asset's history: (re)issuances and burns (an explicit `OP_RETURN`
    /// output of the asset).
    #[serde(default)]
    pub tx_count: u64,
    /// Sum of explicit (re)issued amounts.
    pub issued_amount: u64,
    pub has_blinded_issuances: bool,
    /// Explicit reissuance tokens created.
    pub reissuance_tokens: u64,
    /// From a WFA1 output of the issuance transaction ([`asset_metadata`]); never changed by
    /// reissuances. Absent in records written before these fields existed.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub ticker: Option<String>,
    #[serde(default)]
    pub precision: Option<u8>,
}

/// Tag of an asset-metadata output: `OP_RETURN <"WFA1"> <metadata>`.
pub const WFA1_TAG: &[u8; 4] = b"WFA1";
/// Longest metadata accepted, in bytes.
pub const WFA1_MAX_METADATA: usize = 128;

/// Name, ticker and precision of an asset, from WFA1 metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetMetadata {
    pub name: String,
    pub ticker: String,
    pub precision: u8,
}

/// The metadata pushed by `script`, if it is exactly `OP_RETURN <"WFA1"> <metadata>` with
/// minimal pushes (as `Builder::push_slice` writes them) and at most `WFA1_MAX_METADATA` bytes.
pub fn wfa1_payload(script: &Script) -> Option<&[u8]> {
    const PREFIX: [u8; 6] = [0x6a, 0x04, b'W', b'F', b'A', b'1']; // OP_RETURN, push 4
    let rest = script.as_bytes().strip_prefix(&PREFIX)?;
    let (len, data) = match *rest.first()? {
        n @ 0x01..=0x4b => (n as usize, &rest[1..]),
        0x4c => (*rest.get(1)? as usize, &rest[2..]), // OP_PUSHDATA1
        _ => return None,
    };
    let minimal = rest[0] != 0x4c || len > 0x4b;
    (minimal && data.len() == len && len <= WFA1_MAX_METADATA).then_some(data)
}

/// Parses WFA1 metadata: a UTF-8 JSON object with `name` (1–64 characters after trimming, no
/// control characters), `ticker` (`^[A-Z0-9]{1,8}$`) and `precision` (integer 0–8). Other keys
/// are ignored. The name is returned trimmed.
pub fn parse_asset_metadata(bytes: &[u8]) -> Option<AssetMetadata> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let obj = v.as_object()?;
    let name = obj.get("name")?.as_str()?.trim();
    let chars = name.chars().count();
    if !(1..=64).contains(&chars) || name.chars().any(char::is_control) {
        return None;
    }
    let ticker = obj.get("ticker")?.as_str()?;
    if !(1..=8).contains(&ticker.len()) || !ticker.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()) {
        return None;
    }
    let precision = obj.get("precision")?.as_u64().filter(|p| *p <= 8)? as u8;
    Some(AssetMetadata { name: name.to_owned(), ticker: ticker.to_owned(), precision })
}

/// Metadata naming a new issuance with `contract_hash`: the first output of `tx` that is a
/// valid WFA1 output with an explicit value and whose metadata hashes (SHA-256, raw bytes) to
/// `contract_hash`.
pub fn asset_metadata(tx: &Transaction, contract_hash: &[u8; 32]) -> Option<AssetMetadata> {
    tx.output
        .iter()
        .filter(|o| o.value.is_explicit())
        .filter_map(|o| wfa1_payload(&o.script_pubkey))
        .filter(|m| sha256::Hash::hash(m).to_byte_array() == *contract_hash)
        .find_map(parse_asset_metadata)
}

impl Store {
    /// Last indexed height, `None` before genesis is indexed.
    pub fn indexed_height(&self) -> Result<Option<u64>> {
        Ok(self.meta("indexed_height")?.map(|b| u64::from_le_bytes(b.try_into().unwrap_or_default())))
    }

    /// Indexes the next committed block, if any. Returns its height.
    pub fn index_next(&self) -> Result<Option<u64>> {
        let next = self.indexed_height()?.map_or(0, |h| h + 1);
        if next > self.tip()?.0 {
            return Ok(None);
        }
        let block = self.block(next)?.ok_or(StoreError::Corrupt("indexer: missing block"))?;

        // Gather everything that needs reads before the write transaction.
        let mut history: Vec<[u8; 32]> = Vec::new(); // script hashes, paired with txs below
        let mut rows: Vec<(Vec<u8>, ())> = Vec::new();
        let mut spent: Vec<([u8; 36], Vec<u8>)> = Vec::new();
        let mut assets: Vec<AssetRecord> = Vec::new();
        let mut asset_rows: Vec<Vec<u8>> = Vec::new();
        let mut touched: Vec<AssetId> = Vec::new();
        for (pos, tx) in block.txs.iter().enumerate() {
            let txid = tx.txid();
            history.clear();
            touched.clear();
            for out in &tx.output {
                if !out.script_pubkey.is_empty() {
                    history.push(script_hash(&out.script_pubkey));
                }
            }
            for (vin, input) in tx.input.iter().enumerate() {
                if input.previous_output.is_null() {
                    continue;
                }
                let mut by = txid.to_byte_array().to_vec();
                by.extend_from_slice(&(vin as u32).to_le_bytes());
                spent.push((encode_outpoint(&input.previous_output), by));
                if let Some(prev) = self.output(&input.previous_output)? {
                    history.push(script_hash(&prev.script_pubkey));
                }
                if !input.asset_issuance.is_null() {
                    let record = self.asset_update(tx, vin, next, &assets)?;
                    touched.push(record.asset_id.parse().map_err(|_| StoreError::Corrupt("asset id"))?);
                    assets.push(record);
                }
            }
            // Burns: explicit OP_RETURN outputs of an issued asset.
            for out in &tx.output {
                if let (true, elements::confidential::Asset::Explicit(id)) = (out.script_pubkey.is_op_return(), out.asset) {
                    touched.push(id);
                }
            }
            touched.sort_unstable();
            touched.dedup();
            for id in &touched {
                let existing = match assets.iter().rev().find(|r| r.asset_id == id.to_string()) {
                    Some(r) => Some(r.clone()),
                    None => self.asset(id)?,
                };
                // A burn of an asset never issued on this chain (the fee asset) has no history.
                if let Some(mut record) = existing {
                    record.tx_count += 1;
                    assets.push(record);
                    asset_rows.push(history_key(&id.to_byte_array(), next, pos as u32, &txid));
                }
            }
            history.sort_unstable();
            history.dedup();
            for sh in &history {
                rows.push((history_key(sh, next, pos as u32, &txid), ()));
            }
        }

        let txn = self.db.begin_write().map_err(dberr)?;
        {
            let mut h = txn.open_table(SPK_HISTORY).map_err(dberr)?;
            for (k, _) in &rows {
                h.insert(k.as_slice(), ()).map_err(dberr)?;
            }
            let mut s = txn.open_table(SPENT_BY).map_err(dberr)?;
            for (k, v) in &spent {
                s.insert(k.as_slice(), v.as_slice()).map_err(dberr)?;
            }
            let mut ah = txn.open_table(ASSET_HISTORY).map_err(dberr)?;
            for k in &asset_rows {
                ah.insert(k.as_slice(), ()).map_err(dberr)?;
            }
            let mut a = txn.open_table(ASSETS).map_err(dberr)?;
            for rec in &assets {
                let id: AssetId = rec.asset_id.parse().map_err(|_| StoreError::Corrupt("asset id"))?;
                let json = serde_json::to_vec(rec).map_err(|_| StoreError::Corrupt("asset record"))?;
                a.insert(id.to_byte_array().as_slice(), json.as_slice()).map_err(dberr)?;
            }
            txn.open_table(META).map_err(dberr)?.insert("indexed_height", next.to_le_bytes().as_slice()).map_err(dberr)?;
        }
        txn.commit().map_err(dberr)?;
        Ok(Some(next))
    }

    /// Confirmed transactions touching `script_hash`, newest first: `(height, position, txid)`.
    pub fn script_history(&self, sh: &[u8; 32], before: Option<(u64, u32)>, limit: usize) -> Result<Vec<(u64, u32, Txid)>> {
        self.history_page(SPK_HISTORY, sh, before, limit)
    }

    /// Confirmed transactions in `asset`'s history ((re)issuances and burns), newest first:
    /// `(height, position, txid)`.
    pub fn asset_history(&self, asset: &AssetId, before: Option<(u64, u32)>, limit: usize) -> Result<Vec<(u64, u32, Txid)>> {
        self.history_page(ASSET_HISTORY, &asset.to_byte_array(), before, limit)
    }

    /// Number of confirmed transactions touching `script_hash`.
    pub fn script_tx_count(&self, sh: &[u8; 32]) -> Result<u64> {
        let txn = self.db.begin_read().map_err(dberr)?;
        let table = txn.open_table(SPK_HISTORY).map_err(dberr)?;
        let lo = sh.to_vec();
        let mut hi = sh.to_vec();
        hi.extend_from_slice(&[0xff; 45]);
        let mut n = 0u64;
        for item in table.range::<&[u8]>(lo.as_slice()..hi.as_slice()).map_err(dberr)? {
            item.map_err(dberr)?;
            n += 1;
        }
        Ok(n)
    }

    /// Rows `key (32) ‖ height ‖ position ‖ txid` of `table`, newest first, strictly before
    /// `before` when given.
    fn history_page(
        &self,
        table: TableDefinition<&[u8], ()>,
        key: &[u8; 32],
        before: Option<(u64, u32)>,
        limit: usize,
    ) -> Result<Vec<(u64, u32, Txid)>> {
        let txn = self.db.begin_read().map_err(dberr)?;
        let table = txn.open_table(table).map_err(dberr)?;
        let lo = key.to_vec();
        let mut hi = key.to_vec();
        match before {
            Some((h, p)) => {
                hi.extend_from_slice(&h.to_be_bytes());
                hi.extend_from_slice(&p.to_be_bytes());
            }
            None => hi.extend_from_slice(&[0xff; 12]),
        }
        let mut out = Vec::new();
        for item in table.range::<&[u8]>(lo.as_slice()..hi.as_slice()).map_err(dberr)?.rev() {
            let (k, _) = item.map_err(dberr)?;
            let k = k.value();
            let height = u64::from_be_bytes(k[32..40].try_into().expect("key layout"));
            let pos = u32::from_be_bytes(k[40..44].try_into().expect("key layout"));
            out.push((height, pos, Txid::from_byte_array(k[44..76].try_into().expect("key layout"))));
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    /// Drops the query indexes and their cursor if they were built by another
    /// [`INDEX_VERSION`]; the indexer then rebuilds them from the stored blocks.
    pub fn check_index_version(&self) -> Result<bool> {
        if self.meta("index_version")?.as_deref() == Some(INDEX_VERSION) {
            return Ok(false);
        }
        let txn = self.db.begin_write().map_err(dberr)?;
        {
            txn.delete_table(SPK_HISTORY).map_err(dberr)?;
            txn.delete_table(SPENT_BY).map_err(dberr)?;
            txn.delete_table(ASSETS).map_err(dberr)?;
            txn.delete_table(ASSET_HISTORY).map_err(dberr)?;
            txn.open_table(SPK_HISTORY).map_err(dberr)?;
            txn.open_table(SPENT_BY).map_err(dberr)?;
            txn.open_table(ASSETS).map_err(dberr)?;
            txn.open_table(ASSET_HISTORY).map_err(dberr)?;
            let mut meta = txn.open_table(META).map_err(dberr)?;
            meta.remove("indexed_height").map_err(dberr)?;
            meta.insert("index_version", INDEX_VERSION).map_err(dberr)?;
        }
        txn.commit().map_err(dberr)?;
        Ok(true)
    }

    /// The transaction input that spent `outpoint`, if indexed.
    pub fn spent_by(&self, outpoint: &OutPoint) -> Result<Option<(Txid, u32)>> {
        let txn = self.db.begin_read().map_err(dberr)?;
        let table = txn.open_table(SPENT_BY).map_err(dberr)?;
        Ok(table.get(encode_outpoint(outpoint).as_slice()).map_err(dberr)?.map(|v| {
            let v = v.value();
            (Txid::from_byte_array(v[..32].try_into().expect("layout")), u32::from_le_bytes(v[32..36].try_into().expect("layout")))
        }))
    }

    /// Every indexed asset, in key order (callers sort as they need).
    pub fn assets(&self) -> Result<Vec<AssetRecord>> {
        let txn = self.db.begin_read().map_err(dberr)?;
        let table = txn.open_table(ASSETS).map_err(dberr)?;
        table
            .iter()
            .map_err(dberr)?
            .map(|entry| {
                let (_, value) = entry.map_err(dberr)?;
                serde_json::from_slice(value.value()).map_err(|_| StoreError::Corrupt("asset record"))
            })
            .collect()
    }

    pub fn asset(&self, id: &AssetId) -> Result<Option<AssetRecord>> {
        let txn = self.db.begin_read().map_err(dberr)?;
        let table = txn.open_table(ASSETS).map_err(dberr)?;
        table
            .get(id.to_byte_array().as_slice())
            .map_err(dberr)?
            .map(|v| serde_json::from_slice(v.value()).map_err(|_| StoreError::Corrupt("asset record")))
            .transpose()
    }

    /// A committed output (spent or not), from its transaction.
    pub fn output(&self, op: &OutPoint) -> Result<Option<elements::TxOut>> {
        Ok(self.transaction(&op.txid)?.and_then(|(tx, _)| tx.output.into_iter().nth(op.vout as usize)))
    }

    fn asset_update(&self, tx: &Transaction, vin: usize, height: u64, pending: &[AssetRecord]) -> Result<AssetRecord> {
        let input = &tx.input[vin];
        let iss = &input.asset_issuance;
        let (asset, token) = input.issuance_ids();
        let explicit = |v: Value| match v {
            Value::Explicit(n) => (n, false),
            Value::Confidential(_) => (0, true),
            Value::Null => (0, false),
        };
        let (amount, blinded) = explicit(iss.amount);
        let (tokens, _) = explicit(iss.inflation_keys);
        let existing = match pending.iter().rev().find(|r| r.asset_id == asset.to_string()) {
            Some(r) => Some(r.clone()),
            None => self.asset(&asset)?,
        };
        Ok(match existing {
            Some(mut r) => {
                r.issuance_count += 1;
                r.issued_amount = r.issued_amount.saturating_add(amount);
                r.has_blinded_issuances |= blinded;
                r
            }
            None => {
                let new_issuance = iss.asset_blinding_nonce.is_null();
                let contract = if new_issuance { iss.asset_entropy.to_byte_array() } else { [0; 32] };
                let meta = if new_issuance { asset_metadata(tx, &contract) } else { None };
                let entropy = if new_issuance {
                    AssetId::generate_asset_entropy(input.previous_output, elements::ContractHash::from_byte_array(contract))
                } else {
                    iss.asset_entropy
                };
                AssetRecord {
                    asset_id: asset.to_string(),
                    reissuance_token: token.to_string(),
                    issuance_txid: tx.txid().to_string(),
                    issuance_vin: vin as u32,
                    issuance_prevout: (input.previous_output.txid.to_string(), input.previous_output.vout),
                    contract_hash: hex::encode(contract),
                    entropy: hex::encode(entropy.to_byte_array()),
                    height,
                    issuance_count: 1,
                    tx_count: 0,
                    issued_amount: amount,
                    has_blinded_issuances: blinded,
                    reissuance_tokens: tokens,
                    name: meta.as_ref().map(|m| m.name.clone()),
                    ticker: meta.as_ref().map(|m| m.ticker.clone()),
                    precision: meta.map(|m| m.precision),
                }
            }
        })
    }
}

fn history_key(sh: &[u8; 32], height: u64, pos: u32, txid: &Txid) -> Vec<u8> {
    let mut k = sh.to_vec();
    k.extend_from_slice(&height.to_be_bytes());
    k.extend_from_slice(&pos.to_be_bytes());
    k.extend_from_slice(&txid.to_byte_array());
    k
}

