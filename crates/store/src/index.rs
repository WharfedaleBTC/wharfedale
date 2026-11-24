//! Query indexer for the Esplora API: script history, spent-by, assets. Runs behind the tip
//! with its own cursor (`meta.indexed_height`); never blocks consensus; rebuildable from blocks.

use elements::confidential::Value;
use elements::hashes::sha256;
use elements::{AssetId, OutPoint, Script, Transaction, Txid};
use redb::TableDefinition;
use serde::{Deserialize, Serialize};

use crate::codec::encode_outpoint;
use crate::{dberr, Result, Store, StoreError, META};

/// script_hash (32) ‖ height (u64 BE) ‖ position (u32 BE) ‖ txid (32) → ()
pub(crate) const SPK_HISTORY: TableDefinition<&[u8], ()> = TableDefinition::new("spk_history");
/// outpoint (36) → spending txid (32) ‖ vin (u32 LE)
pub(crate) const SPENT_BY: TableDefinition<&[u8], &[u8]> = TableDefinition::new("spent_by");
/// asset id (32) → JSON `AssetRecord`
pub(crate) const ASSETS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("assets");

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
    /// Sum of explicit (re)issued amounts.
    pub issued_amount: u64,
    pub has_blinded_issuances: bool,
    /// Explicit reissuance tokens created.
    pub reissuance_tokens: u64,
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
        for (pos, tx) in block.txs.iter().enumerate() {
            let txid = tx.txid();
            history.clear();
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
                    assets.push(self.asset_update(tx, vin, next, &assets)?);
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
        let txn = self.db.begin_read().map_err(dberr)?;
        let table = txn.open_table(SPK_HISTORY).map_err(dberr)?;
        let lo = sh.to_vec();
        let mut hi = sh.to_vec();
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

    /// The transaction input that spent `outpoint`, if indexed.
    pub fn spent_by(&self, outpoint: &OutPoint) -> Result<Option<(Txid, u32)>> {
        let txn = self.db.begin_read().map_err(dberr)?;
        let table = txn.open_table(SPENT_BY).map_err(dberr)?;
        Ok(table.get(encode_outpoint(outpoint).as_slice()).map_err(dberr)?.map(|v| {
            let v = v.value();
            (Txid::from_byte_array(v[..32].try_into().expect("layout")), u32::from_le_bytes(v[32..36].try_into().expect("layout")))
        }))
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
                let contract = if iss.asset_blinding_nonce.is_null() { iss.asset_entropy.to_byte_array() } else { [0; 32] };
                let entropy = if iss.asset_blinding_nonce.is_null() {
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
                    issued_amount: amount,
                    has_blinded_issuances: blinded,
                    reissuance_tokens: tokens,
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

