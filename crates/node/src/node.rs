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
    ConsensusContext, EngineBuilder, EngineHandle, NetworkContext, NetworkIdentity, RequestContext, SyncContext,
    WalContext,
};
use malachitebft_signing::Signer as _;
use tokio::task::JoinHandle;
use tracing::{error, Instrument};
use wharfedale_chain::Genesis;
use wharfedale_consensus_ctx::codec::ProtobufCodec;
use wharfedale_consensus_ctx::{validator_set, Address, Ed25519Signer, Ed25519Verifier};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use wharfedale_ledger::VerifyCache;
use wharfedale_mempool::{Mempool, Policy};
use wharfedale_rpc::NodeState;
use wharfedale_store::Store;

use crate::gossip;

use crate::app::{self, App};
use crate::config::NodeConfig;
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
    /// Gossip and RPC tasks.
    pub tasks: Vec<JoinHandle<()>>,
}

impl NodeHandle {
    /// Stops the engine and the application task and waits for both to exit, so the WAL and
    /// the database are released.
    pub async fn shutdown(self) {
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
    }
}

/// Starts a validator (or full node when `keys/validator.json` is absent) from `home`.
pub async fn start(home: &Path) -> Result<NodeHandle> {
    let home = Home(home.to_path_buf());
    let config = NodeConfig::load(home.config())?;
    let genesis = Genesis::load(home.genesis()).wrap_err("loading genesis.json")?;
    let validator_set = validator_set(&genesis.validator_keys()?).map_err(|e| eyre!(e))?;

    std::fs::create_dir_all(home.store().parent().expect("data dir"))?;
    let store = Store::open(home.store())?;
    store.init_or_check(&genesis)?;

    let secret = keys::load(home.validator_key()).wrap_err("loading keys/validator.json")?;
    let private_key = keys::private_key(secret);
    let public_key = private_key.public_key();
    let address = Address::from_public_key(&public_key);
    if validator_set.get_by_address(&address).is_none() {
        return Err(eyre!("validator key {address} is not in genesis"));
    }

    let fee_script = Script::from(hex::decode(&config.fee_payout_script).wrap_err("fee_payout_script")?);
    // Coinbase outputs to anything else would burn the fees (or be rejected as non-standard).
    if !fee_script.is_v1_p2tr() {
        return Err(eyre!("fee_payout_script must be a taproot output script (OP_1 <32 bytes>)"));
    }
    let node_secret = keys::load(home.node_key()).wrap_err("loading keys/node.json")?;
    let keypair = Keypair::ed25519_from_bytes(node_secret).map_err(|e| eyre!("node key: {e}"))?;

    let signer = Ed25519Signer::new(private_key.clone());
    let proof = signer
        .sign_validator_proof(public_key.as_bytes().to_vec(), keypair.public().to_peer_id().to_bytes())
        .await
        .map_err(|e| eyre!("validator proof: {e:?}"))?;
    let proof_bytes = ProtobufCodec.encode(&proof)?;
    let identity = NetworkIdentity::new_validator(config.moniker.clone(), keypair, address.to_string(), proof_bytes);

    let engine_config = config.engine_config()?;
    std::fs::create_dir_all(home.wal().parent().expect("wal dir"))?;
    let (mut channels, engine) = EngineBuilder::new(wharfedale_consensus_ctx::WharfedaleContext, engine_config)
        .with_default_wal(WalContext::new(home.wal(), ProtobufCodec))
        .with_default_network(NetworkContext::new(identity, ProtobufCodec))
        .with_default_consensus(ConsensusContext::new_validator(
            address,
            Box::new(Ed25519Verifier),
            Box::new(Ed25519Signer::new(private_key)),
        ))
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
        mempool: Arc::new(Mutex::new(Mempool::new(Policy::new(fee_asset)))),
        cache: Arc::new(VerifyCache::new()),
        chain_id: genesis.chain_id.clone(),
        fee_asset,
        coinbase_maturity: app::COINBASE_MATURITY,
        relay: relay_tx,
    };
    let span = tracing::error_span!("node", moniker = %config.moniker);

    let mut tasks = Vec::new();
    // Gossip in: batches from the Mempool channel → admission.
    let inbound = std::mem::replace(&mut channels.mempool, mpsc::channel(1).1);
    tasks.push(tokio::spawn(gossip::receive(inbound, node_state.clone()).instrument(span.clone())));
    // Gossip out: locally submitted transactions → batched publish.
    tasks.push(tokio::spawn(gossip::relay(relay_rx, channels.network.clone()).instrument(span.clone())));
    // Query indexer (Esplora): follows the committed tip.
    {
        let st = node_state.clone();
        let mut tip_rx = tip.clone();
        tasks.push(tokio::spawn(
            async move {
                loop {
                    let store = st.store.clone();
                    match tokio::task::spawn_blocking(move || -> Result<(), String> {
                        while store.index_next().map_err(|e| e.to_string())?.is_some() {}
                        Ok(())
                    })
                    .await
                    {
                        Ok(Err(e)) => error!("indexer: {e}"),
                        Err(e) => error!("indexer task: {e}"),
                        Ok(Ok(())) => {}
                    }
                    if tip_rx.changed().await.is_err() {
                        break;
                    }
                }
            }
            .instrument(span.clone()),
        ));
    }
    // RPC.
    if let Some(rpc) = &config.rpc {
        let addr = rpc.listen_addr.parse().wrap_err("rpc.listen_addr")?;
        let st = node_state.clone();
        tasks.push(tokio::spawn(
            async move {
                if let Err(e) = wharfedale_rpc::serve(addr, st).await {
                    error!("RPC server stopped: {e}");
                }
            }
            .instrument(span.clone()),
        ));
    }

    let mut state = App::new(node_state, validator_set, address, signer, fee_script, config.block_time(), tip_tx)?;
    let app = tokio::spawn(
        async move {
            if let Err(e) = app::run(&mut state, &mut channels).await {
                error!("application stopped: {e}");
            }
        }
        .instrument(span),
    );
    Ok(NodeHandle { app, engine, tip, tasks })
}
