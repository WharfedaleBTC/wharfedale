//! Node services shared by RPC and gossip, and the Elements-compatible JSON-RPC server.
//!
//! Spec: `docs/src/rpc.md`.
#![forbid(unsafe_code)]

mod esplora;
mod methods;
mod server;

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use elements::{AssetId, BlockHash, Transaction, Txid};
use tokio::sync::mpsc;
use wharfedale_ledger::{ChainCtx, VerifyCache};
use wharfedale_mempool::{AdmitError, Mempool, Source};
use wharfedale_store::Store;

pub use server::serve;

/// What the node does in consensus (`config.toml` `role`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeRole {
    Validator,
    Full,
}

impl NodeRole {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeRole::Validator => "validator",
            NodeRole::Full => "full",
        }
    }
}

/// The p2p layer as seen by `getnetworkinfo`.
#[derive(Clone, Debug, PartialEq)]
pub struct NetworkInfo {
    /// libp2p peer id of this node.
    pub peer_id: String,
    pub moniker: String,
    /// Lowercase hex; `None` on full nodes.
    pub consensus_address: Option<String>,
    /// Multiaddrs.
    pub listen_addrs: Vec<String>,
    pub peers: Vec<PeerInfo>,
}

/// A connected peer.
#[derive(Clone, Debug, PartialEq)]
pub struct PeerInfo {
    pub peer_id: String,
    pub moniker: String,
    /// Multiaddr.
    pub address: String,
    /// Proved a key in the validator set.
    pub validator: bool,
    /// Lowercase hex, when the peer is a validator.
    pub consensus_address: Option<String>,
    /// `"inbound"` or `"outbound"`.
    pub direction: Option<String>,
    /// GossipSub score.
    pub score: f64,
}

/// Asks the p2p layer for its state (set by the node; the rpc crate does not know Malachite).
pub type NetworkInfoFn =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<NetworkInfo, String>> + Send>> + Send + Sync>;

/// Handles shared by the consensus loop, the gossip receiver and the RPC server.
#[derive(Clone)]
pub struct NodeState {
    pub store: Arc<Store>,
    pub mempool: Arc<Mutex<Mempool>>,
    pub cache: Arc<VerifyCache>,
    pub chain_id: String,
    pub genesis_hash: BlockHash,
    pub fee_asset: AssetId,
    pub coinbase_maturity: u64,
    /// Locally submitted transactions to publish on the Mempool channel.
    pub relay: mpsc::Sender<Transaction>,
    pub role: NodeRole,
    /// `None`: no p2p layer (`getnetworkinfo` fails).
    pub network_info: Option<NetworkInfoFn>,
}

impl NodeState {
    /// Context of the next block (tip + 1), against which the mempool admits.
    pub fn next_block_ctx(&self) -> Result<ChainCtx, String> {
        let (tip, _) = self.store.tip().map_err(|e| e.to_string())?;
        Ok(ChainCtx {
            height: tip + 1,
            median_time_past: self.store.mtp(tip).map_err(|e| e.to_string())?,
            genesis_hash: self.genesis_hash,
            coinbase_maturity: self.coinbase_maturity,
        })
    }

    /// The mempool. A panic while it was held leaves it consistent (every mutation completes
    /// before anything can panic), so a poisoned lock is taken over.
    pub fn pool(&self) -> MutexGuard<'_, Mempool> {
        self.mempool.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Checks a transaction as `admit` would, without adding it (`testmempoolaccept`).
    pub fn test_accept(&self, tx: Transaction) -> Result<Txid, AdmitError> {
        let ctx = self.next_block_ctx().map_err(AdmitError::Unavailable)?;
        let candidate = self.pool().precheck(tx, Source::Local, &*self.store)?;
        if let Err(e) = wharfedale_mempool::verify(&candidate, &*self.store, &ctx, &self.cache) {
            self.pool().record_failure(candidate.wtxid, &e);
            return Err(e.into());
        }
        Ok(candidate.txid)
    }

    /// Admits a transaction; local ones are queued for gossip. Verification runs without the
    /// mempool lock.
    pub fn admit(&self, tx: Transaction, source: Source) -> Result<Txid, AdmitError> {
        let ctx = self.next_block_ctx().map_err(AdmitError::Unavailable)?;
        let relay = (source == Source::Local).then(|| tx.clone());
        let candidate = self.pool().precheck(tx, source, &*self.store)?;
        if let Err(e) = wharfedale_mempool::verify(&candidate, &*self.store, &ctx, &self.cache) {
            self.pool().record_failure(candidate.wtxid, &e);
            return Err(e.into());
        }
        let txid = self.pool().insert(candidate, &*self.store, Instant::now())?;
        if let Some(tx) = relay {
            if self.relay.try_send(tx).is_err() {
                tracing::warn!(%txid, "relay queue full; transaction not gossiped");
            }
        }
        Ok(txid)
    }
}
