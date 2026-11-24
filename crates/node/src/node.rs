//! Node home directory, startup and engine wiring.
//!
//! Home layout:
//! ```text
//! <home>/config.toml
//! <home>/genesis.json
//! <home>/keys/validator.json   consensus key (absent on full nodes)
//! <home>/keys/node.json        libp2p identity
//! <home>/data/store.redb
//! <home>/data/wal/consensus.wal
//! ```

use std::path::{Path, PathBuf};

use elements::Script;
use eyre::{eyre, Result, WrapErr};
use malachitebft_app_channel::app::types::codec::Codec;
use malachitebft_app_channel::app::types::Keypair;

use malachitebft_app_channel::{
    ConsensusContext, EngineBuilder, EngineHandle, NetworkContext, NetworkIdentity, NetworkRequest, RequestContext,
    SyncContext, WalContext,
};
use malachitebft_signing::Signer as _;
use tokio::task::JoinHandle;
use tracing::{error, Instrument};
use wharfedale_chain::Genesis;
use wharfedale_consensus_ctx::codec::ProtobufCodec;
use wharfedale_consensus_ctx::{validator_set, Address, Ed25519Signer, Ed25519Verifier};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use wharfedale_ledger::VerifyCache;
use wharfedale_mempool::{Mempool, Policy};
use wharfedale_rpc::{NetworkInfo, NetworkInfoFn, NodeRole, NodeState, PeerInfo};
use wharfedale_store::Store;

use crate::gossip;

use crate::app::{self, App, Proposer};
use crate::config::{NodeConfig, Role};
use crate::keys;

pub struct Home(pub PathBuf);

impl Home {
    pub fn config(&self) -> PathBuf {
        self.0.join("config.toml")
    }
    pub fn genesis(&self) -> PathBuf {
        self.0.join("genesis.json")
    }
    pub fn validator_key(&self) -> PathBuf {
        self.0.join("keys").join("validator.json")
    }
    pub fn node_key(&self) -> PathBuf {
        self.0.join("keys").join("node.json")
    }
    pub fn store(&self) -> PathBuf {
        self.0.join("data").join("store.redb")
    }
    pub fn wal(&self) -> PathBuf {
        self.0.join("data").join("wal").join("consensus.wal")
    }
}

/// A running node: the application task and the engine.
pub struct NodeHandle {
    pub app: JoinHandle<()>,
    pub engine: EngineHandle,
    /// Committed tip height.
    pub tip: tokio::sync::watch::Receiver<u64>,
    /// Gossip and indexer tasks.
    pub tasks: Vec<JoinHandle<()>>,
    /// The RPC server, stopped gracefully: its open connections hold the database.
    rpc: Option<(tokio::sync::oneshot::Sender<()>, JoinHandle<()>)>,
    /// The indexer runs blocking work that an abort can't interrupt: stopped by flag, awaited.
    indexer: (Arc<AtomicBool>, JoinHandle<()>),
}

impl NodeHandle {
    /// Stops the engine and the application task and waits for both to exit, so the WAL and
    /// the database are released.
    pub async fn shutdown(self) {
        if let Some((stop, task)) = self.rpc {
            let _ = stop.send(());
            if tokio::time::timeout(std::time::Duration::from_secs(10), task).await.is_err() {
                tracing::warn!("RPC server did not stop within 10 s");
            }
        }
        self.engine.actor.stop(Some("shutdown".into()));
        let _ = tokio::time::timeout(std::time::Duration::from_secs(10), self.engine.handle).await;
        for task in &self.tasks {
            task.abort();
        }
        self.app.abort();
        let _ = self.app.await;
        for task in self.tasks {
            let _ = task.await;
        }
        // The app is gone, so the tip channel is closed: the indexer exits after its current
        // block, releasing its handle on the database.
        let (stop, indexer) = self.indexer;
        stop.store(true, Ordering::Relaxed);
        if tokio::time::timeout(std::time::Duration::from_secs(30), indexer).await.is_err() {
            tracing::warn!("indexer did not stop within 30 s");
        }
    }
}

/// `getnetworkinfo` source: a state dump from the network actor.
fn network_info(requests: mpsc::Sender<NetworkRequest>) -> NetworkInfoFn {
    Arc::new(move || {
        let requests = requests.clone();
        Box::pin(async move {
            let dump = NetworkRequest::dump_state(&requests)
                .await
                .map_err(|e| format!("network state request failed: {e}"))?
                .ok_or("network state unavailable")?;
            let local = dump.local_node;
            Ok(NetworkInfo {
                peer_id: local.peer_id.to_string(),
                moniker: local.moniker,
                consensus_address: local.consensus_address.map(|a| a.to_ascii_lowercase()),
                listen_addrs: vec![local.listen_addr.to_string()],
                peers: dump
                    .peers
                    .into_iter()
                    .map(|(id, p)| PeerInfo {
                        peer_id: id.to_string(),
                        moniker: p.moniker,
                        address: p.address.to_string(),
                        validator: p.consensus_address.is_some() || p.peer_type.is_validator(),
                        consensus_address: p.consensus_address.map(|a| a.to_ascii_lowercase()),
                        direction: p.connection_direction.map(|d| d.as_str().to_owned()),
                        score: p.score,
                    })
                    .collect(),
            })
        })
    })
}

/// Starts a node from `home`: a validator or a full node, by `config.role`.
pub async fn start(home: &Path) -> Result<NodeHandle> {
    let home = Home(home.to_path_buf());
    let config = NodeConfig::load(home.config())?;
    let genesis = Genesis::load(home.genesis()).wrap_err("loading genesis.json")?;
    let validator_set = validator_set(&genesis.validator_keys()?).map_err(|e| eyre!(e))?;

    std::fs::create_dir_all(home.store().parent().expect("data dir"))?;
    let store = Store::open(home.store())?;
    store.init_or_check(&genesis)?;

    let node_secret = keys::load(home.node_key()).wrap_err("loading keys/node.json")?;
    let keypair = Keypair::ed25519_from_bytes(node_secret).map_err(|e| eyre!("node key: {e}"))?;

    let (address, identity, consensus, proposer) = match config.role {
        Role::Validator => {
            let secret = keys::load(home.validator_key()).wrap_err("loading keys/validator.json")?;
            let private_key = keys::private_key(secret);
            let public_key = private_key.public_key();
            let address = Address::from_public_key(&public_key);
            if validator_set.get_by_address(&address).is_none() {
                return Err(eyre!("validator key {address} is not in genesis (set role = \"full\" to run a full node)"));
            }
            let fee_script = Script::from(hex::decode(&config.fee_payout_script).wrap_err("fee_payout_script")?);
            // Coinbase outputs to anything else would burn the fees (or be rejected as non-standard).
            if !fee_script.is_v1_p2tr() {
                return Err(eyre!("fee_payout_script must be a taproot output script (OP_1 <32 bytes>)"));
            }
            let signer = Ed25519Signer::new(private_key.clone());
            let proof = signer
                .sign_validator_proof(public_key.as_bytes().to_vec(), keypair.public().to_peer_id().to_bytes())
                .await
                .map_err(|e| eyre!("validator proof: {e:?}"))?;
            let proof_bytes = ProtobufCodec.encode(&proof)?;
            let identity = NetworkIdentity::new_validator(config.moniker.clone(), keypair, address.to_string(), proof_bytes);
            let consensus = ConsensusContext::new_validator(address, Box::new(Ed25519Verifier), Box::new(Ed25519Signer::new(private_key)));
            (address, identity, consensus, Some(Proposer { signer, fee_script }))
        }
        Role::Full => {
            // Consensus needs an address; derived from the node key, it is in no validator
            // set, so this node is never selected to propose and never signs.
            let address = Address::from_public_key(&keys::private_key(node_secret).public_key());
            if validator_set.get_by_address(&address).is_some() {
                return Err(eyre!("full node key collides with a validator"));
            }
            let identity = NetworkIdentity::new(config.moniker.clone(), keypair, None);
            let consensus = ConsensusContext::new_full_node(address, Box::new(Ed25519Verifier));
            (address, identity, consensus, None)
        }
    };

    let engine_config = config.engine_config()?;
    std::fs::create_dir_all(home.wal().parent().expect("wal dir"))?;
    let (mut channels, engine) = EngineBuilder::new(wharfedale_consensus_ctx::WharfedaleContext, engine_config)
        .with_default_wal(WalContext::new(home.wal(), ProtobufCodec))
        .with_default_network(NetworkContext::new(identity, ProtobufCodec))
        .with_default_consensus(consensus)
        .with_default_sync(SyncContext::new(ProtobufCodec))
        .with_default_request(RequestContext::new(100))
        .build()
        .await?;

    let (tip_tx, tip) = tokio::sync::watch::channel(0);
    let fee_asset = genesis.fee_asset()?;
    let (relay_tx, relay_rx) = mpsc::channel(10_000);
    let node_state = NodeState {
        genesis_hash: store.genesis_hash()?,
        store: Arc::new(store),
        mempool: Arc::new(Mutex::new(Mempool::new(Policy { min_fee_per_kvb: config.min_fee_per_kvb, ..Policy::new(fee_asset) }))),
        cache: Arc::new(VerifyCache::new()),
        chain_id: genesis.chain_id.clone(),
        fee_asset,
        coinbase_maturity: app::COINBASE_MATURITY,
        relay: relay_tx,
        role: match config.role {
            Role::Validator => NodeRole::Validator,
            Role::Full => NodeRole::Full,
        },
        network_info: Some(network_info(channels.net_requests.clone())),
    };
    let span = tracing::error_span!("node", moniker = %config.moniker);

    let mut tasks = Vec::new();
    // Gossip in: batches from the Mempool channel → admission.
    let inbound = std::mem::replace(&mut channels.mempool, mpsc::channel(1).1);
    tasks.push(tokio::spawn(gossip::receive(inbound, node_state.clone()).instrument(span.clone())));
    // Gossip out: locally submitted transactions → batched publish.
    tasks.push(tokio::spawn(gossip::relay(relay_rx, channels.network.clone()).instrument(span.clone())));
    // Query indexer (Esplora): follows the committed tip.
    let indexer_stop = Arc::new(AtomicBool::new(false));
    let indexer = {
        let st = node_state.clone();
        let mut tip_rx = tip.clone();
        let stop = indexer_stop.clone();
        tokio::spawn(
            async move {
                loop {
                    let store = st.store.clone();
                    let stop_now = stop.clone();
                    match tokio::task::spawn_blocking(move || -> Result<(), String> {
                        while !stop_now.load(Ordering::Relaxed) && store.index_next().map_err(|e| e.to_string())?.is_some() {}
                        Ok(())
                    })
                    .await
                    {
                        Ok(Err(e)) => error!("indexer: {e}"),
                        Err(e) => error!("indexer task: {e}"),
                        Ok(Ok(())) => {}
                    }
                    if stop.load(Ordering::Relaxed) || tip_rx.changed().await.is_err() {
                        break;
                    }
                }
            }
            .instrument(span.clone()),
        )
    };
    // RPC.
    let mut rpc_task = None;
    if let Some(rpc) = &config.rpc {
        let addr = rpc.listen_addr.parse().wrap_err("rpc.listen_addr")?;
        let st = node_state.clone();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(
            async move {
                if let Err(e) = wharfedale_rpc::serve(addr, st, async move { let _ = stopped.await; }).await {
                    error!("RPC server stopped: {e}");
                }
            }
            .instrument(span.clone()),
        );
        rpc_task = Some((stop, task));
    }

    let mut state = App::new(node_state, validator_set, address, proposer, config.block_time(), config.create_empty_blocks, tip_tx)?;
    let app = tokio::spawn(
        async move {
            if let Err(e) = app::run(&mut state, &mut channels).await {
                error!("application stopped: {e}");
            }
        }
        .instrument(span),
    );
    Ok(NodeHandle { app, engine, tip, tasks, rpc: rpc_task, indexer: (indexer_stop, indexer) })
}
