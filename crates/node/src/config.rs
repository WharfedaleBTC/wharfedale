//! `config.toml` (per node) and its translation into Malachite's configuration.

use std::path::Path;
use std::time::Duration;

use eyre::{Context as _, Result};
use malachitebft_app_channel::app::config::NodeConfig as MalachiteNodeConfig;
use malachitebft_config::{ConsensusConfig, DiscoveryConfig, P2pConfig, ValuePayload, ValueSyncConfig};
use serde::{Deserialize, Serialize};

/// What a node does in consensus.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Proposes and votes with `keys/validator.json`, which must be in genesis.
    #[default]
    Validator,
    /// Follows the chain (consensus gossip and value sync), validates every block, keeps the
    /// full state, serves RPC and relays transactions; never proposes or votes. Needs no
    /// validator key or fee script.
    Full,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeConfig {
    pub moniker: String,
    #[serde(default)]
    pub role: Role,
    /// Target time between blocks (Malachite `target_time`).
    #[serde(default = "default_block_time_ms")]
    pub block_time_ms: u64,
    /// Hex taproot scriptPubKey (`5120…`) the coinbase pays fees to; checked at startup on
    /// validators, unused on full nodes.
    #[serde(default)]
    pub fee_payout_script: String,
    /// Build blocks with no transactions. Off (the default): after a height is finalized the
    /// node waits for a transaction (or a peer's proposal for the next height) before starting
    /// the next one, so the chain only grows when there is something to include.
    #[serde(default)]
    pub create_empty_blocks: bool,
    /// Lowest fee rate the mempool accepts, in fee-asset units per 1 000 vbytes.
    #[serde(default = "default_min_fee_per_kvb")]
    pub min_fee_per_kvb: u64,
    pub p2p: P2p,
    /// JSON-RPC server; absent = disabled.
    #[serde(default)]
    pub rpc: Option<Rpc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rpc {
    /// e.g. `127.0.0.1:18443`
    pub listen_addr: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct P2p {
    /// Multiaddr, e.g. `/ip4/0.0.0.0/tcp/27000`.
    pub listen_addr: String,
    #[serde(default)]
    pub persistent_peers: Vec<String>,
    /// Connect only to `persistent_peers`.
    #[serde(default = "default_true")]
    pub persistent_peers_only: bool,
}

fn default_min_fee_per_kvb() -> u64 {
    wharfedale_mempool::DEFAULT_MIN_FEE_PER_KVB
}

fn default_block_time_ms() -> u64 {
    1_000
}

fn default_true() -> bool {
    true
}

impl NodeConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let raw = std::fs::read_to_string(path.as_ref()).wrap_err("reading config.toml")?;
        toml::from_str(&raw).wrap_err("parsing config.toml")
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn block_time(&self) -> Duration {
        Duration::from_millis(self.block_time_ms)
    }

    /// Malachite configuration: parts-based value propagation, persistent peers only, no
    /// discovery, value sync on.
    pub fn engine_config(&self) -> Result<EngineConfig> {
        let mut p2p = P2pConfig {
            persistent_peers_only: self.p2p.persistent_peers_only,
            enable_mempool: true,
            discovery: DiscoveryConfig { enabled: false, max_connections_per_ip: usize::MAX, ..Default::default() },
            ..Default::default()
        };
        p2p.listen_addr = self.p2p.listen_addr.parse().wrap_err("p2p.listen_addr")?;
        p2p.persistent_peers = self
            .p2p
            .persistent_peers
            .iter()
            .map(|a| a.parse())
            .collect::<std::result::Result<_, _>>()
            .wrap_err("p2p.persistent_peers")?;
        Ok(EngineConfig {
            moniker: self.moniker.clone(),
            consensus: ConsensusConfig {
                value_payload: ValuePayload::ProposalAndParts,
                queue_capacity: 100,
                p2p,
                ..Default::default()
            },
            value_sync: ValueSyncConfig {
                enabled: true,
                status_update_interval: Duration::from_secs(2),
                request_timeout: Duration::from_secs(5),
                ..Default::default()
            },
        })
    }
}

/// The configuration handed to `EngineBuilder`.
#[derive(Clone, Debug)]
pub struct EngineConfig {
    pub moniker: String,
    pub consensus: ConsensusConfig,
    pub value_sync: ValueSyncConfig,
}

impl MalachiteNodeConfig for EngineConfig {
    fn moniker(&self) -> &str {
        &self.moniker
    }
    fn consensus(&self) -> &ConsensusConfig {
        &self.consensus
    }
    fn consensus_mut(&mut self) -> &mut ConsensusConfig {
        &mut self.consensus
    }
    fn value_sync(&self) -> &ValueSyncConfig {
        &self.value_sync
    }
    fn value_sync_mut(&mut self) -> &mut ValueSyncConfig {
        &mut self.value_sync
    }
}
