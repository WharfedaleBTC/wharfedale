//! `genesis.json`: chain id, fixed validator set, initial transactions.

use std::path::Path;

use elements::{encode, OutPoint};
use serde::{Deserialize, Serialize};
use wharfedale_ledger::{output_kind, BlockDiff, OutputKind, UtxoEntry};

use crate::block::Block;
use crate::header::Header;
use crate::{merkle, sha256d, Hash32};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenesisValidator {
    /// Ed25519 public key, hex (32 bytes).
    pub public_key: String,
    pub voting_power: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Genesis {
    pub chain_id: String,
    /// Unix seconds; the genesis header timestamp.
    pub genesis_time: u64,
    /// The complete, permanent validator set.
    pub validators: Vec<GenesisValidator>,
    /// Asset id (hex) counted by mempool fee policy. Normally `fee_asset_for(chain_id)`.
    pub fee_asset: String,
    /// Hex-encoded Elements transactions applied without validation.
    #[serde(default)]
    pub initial_txs: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum GenesisError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("validator {0}: public key must be 32 bytes of hex")]
    PublicKey(usize),
    #[error("validator {0}: voting power must be positive")]
    VotingPower(usize),
    #[error("validator {0}: public key listed twice")]
    DuplicateValidator(usize),
    #[error("no validators")]
    NoValidators,
    #[error("initial transaction {0} is not valid hex-encoded Elements")]
    InitialTx(usize),
    #[error("fee_asset is not a valid asset id")]
    FeeAsset,
}

impl Genesis {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, GenesisError> {
        let genesis: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        genesis.validator_keys()?;
        genesis.fee_asset()?;
        Ok(genesis)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), GenesisError> {
        std::fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    pub fn fee_asset(&self) -> Result<elements::AssetId, GenesisError> {
        self.fee_asset.parse().map_err(|_| GenesisError::FeeAsset)
    }

    /// `(ed25519 public key, voting power)` in genesis order.
    pub fn validator_keys(&self) -> Result<Vec<([u8; 32], u64)>, GenesisError> {
        if self.validators.is_empty() {
            return Err(GenesisError::NoValidators);
        }
        let mut seen = std::collections::HashSet::with_capacity(self.validators.len());
        self.validators
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let key: [u8; 32] = hex::decode(&v.public_key)
                    .ok()
                    .and_then(|b| b.try_into().ok())
                    .ok_or(GenesisError::PublicKey(i))?;
                if v.voting_power == 0 {
                    return Err(GenesisError::VotingPower(i));
                }
                // A repeated key would count its power twice in the total but once in votes
                // (looked up by address): quorums could become unreachable.
                if !seen.insert(key) {
                    return Err(GenesisError::DuplicateValidator(i));
                }
                Ok((key, v.voting_power))
            })
            .collect()
    }

    /// Commits to the chain id and the validator set; becomes the genesis `prev_block_hash`,
    /// so the genesis hash identifies the chain.
    pub fn params_hash(&self) -> Result<Hash32, GenesisError> {
        let mut buf = b"wharfedale/genesis".to_vec();
        buf.extend_from_slice(&(self.chain_id.len() as u32).to_le_bytes());
        buf.extend_from_slice(self.chain_id.as_bytes());
        buf.extend_from_slice(&self.fee_asset()?.to_byte_array());
        let keys = self.validator_keys()?;
        buf.extend_from_slice(&(keys.len() as u32).to_le_bytes());
        for (key, power) in keys {
            buf.extend_from_slice(&key);
            buf.extend_from_slice(&power.to_le_bytes());
        }
        Ok(sha256d(&buf))
    }

    /// The genesis block (height 0).
    pub fn block(&self) -> Result<Block, GenesisError> {
        let txs = self
            .initial_txs
            .iter()
            .enumerate()
            .map(|(i, h)| {
                hex::decode(h)
                    .ok()
                    .and_then(|b| encode::deserialize(&b).ok())
                    .ok_or(GenesisError::InitialTx(i))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let header = Header {
            version: Header::VERSION,
            height: 0,
            prev_block_hash: self.params_hash()?,
            timestamp: self.genesis_time,
            proposer: [0; 20],
            tx_merkle_root: merkle::tx_root(&txs),
            witness_merkle_root: merkle::witness_root(&txs),
        };
        Ok(Block { header, txs })
    }
}

/// UTXOs created by genesis: every taproot output of the initial transactions, at height 0.
pub fn genesis_diff(block: &Block) -> BlockDiff {
    let mut diff = BlockDiff::default();
    for tx in &block.txs {
        let txid = tx.txid();
        for (vout, out) in tx.output.iter().enumerate() {
            if output_kind(out) == Some(OutputKind::Taproot) {
                diff.created.push((OutPoint::new(txid, vout as u32), UtxoEntry::new(out.clone(), 0, false)));
            }
        }
    }
    diff
}
