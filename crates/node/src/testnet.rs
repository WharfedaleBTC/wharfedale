//! Testnet generation: validator and full-node homes sharing one genesis, plus a funded faucet
//! key.
//!
//! Output under `root`:
//! ```text
//! root/faucet.json              faucet key ({"secret": hex}); genesis pays it FAUCET_OUTPUTS coins
//! root/node-<i>/...             one home per validator (see `node::Home`)
//! root/node-<i>/keys/fee.json   key of that validator's coinbase payout script
//! root/full-<i>/...             one home per full node (role = "full", no validator key)
//! ```
//!
//! Every node lists every other as a persistent peer (`persistent_peers_only`), so validators
//! accept their full nodes.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use eyre::Result;
use malachitebft_app_channel::app::types::Keypair;
use rand::{CryptoRng, RngCore};
use wharfedale_chain::{fee_asset_for, genesis_funding_tx, key_path_script, Genesis, GenesisValidator};

use crate::config::{NodeConfig, P2p, Role, Rpc};
use crate::keys;
use crate::node::Home;

/// Genesis pays the faucet this many outputs of `FAUCET_OUTPUT_VALUE` fee-asset units each,
/// so the faucet can fund many transactions per block (v1 mempool: confirmed inputs only).
pub const FAUCET_OUTPUTS: usize = 20;
pub const FAUCET_OUTPUT_VALUE: u64 = 1_000_000_000_000;

/// Network placement of one node.
#[derive(Clone, Debug)]
pub struct NodeAddr {
    /// Multiaddr the node listens on, e.g. `/ip4/0.0.0.0/tcp/27000`.
    pub listen: String,
    /// Multiaddr prefix peers dial, e.g. `/ip4/127.0.0.1/tcp/27000` or `/dns4/node0/tcp/27000`.
    pub dial: String,
    /// RPC listen address, if any.
    pub rpc: Option<String>,
}

/// `n` nodes on 127.0.0.1: P2P on `base_port + i`, RPC on `rpc_base + i`.
pub fn local(n: usize, base_port: u16, rpc_base: Option<u16>) -> Vec<NodeAddr> {
    (0..n as u16)
        .map(|i| NodeAddr {
            listen: format!("/ip4/127.0.0.1/tcp/{}", base_port + i),
            dial: format!("/ip4/127.0.0.1/tcp/{}", base_port + i),
            rpc: rpc_base.map(|b| format!("127.0.0.1:{}", b + i)),
        })
        .collect()
}

/// Containers named `hosts[i]`, each listening on `port` and serving RPC on `rpc_port`.
pub fn docker(hosts: &[String], port: u16, rpc_port: u16) -> Vec<NodeAddr> {
    hosts
        .iter()
        .map(|h| NodeAddr {
            listen: format!("/ip4/0.0.0.0/tcp/{port}"),
            dial: format!("/dns4/{h}/tcp/{port}"),
            rpc: Some(format!("0.0.0.0:{rpc_port}")),
        })
        .collect()
}

/// Writes one validator home per entry of `nodes` and the faucet key. Returns the homes.
pub fn generate<R: RngCore + CryptoRng>(root: &Path, nodes: &[NodeAddr], chain_id: &str, rng: &mut R) -> Result<Vec<PathBuf>> {
    let (validators, _) = generate_network(root, nodes, &[], chain_id, rng)?;
    Ok(validators)
}

/// Validator homes (`node-<i>`) and full-node homes (`full-<i>`), fully meshed. Returns both.
pub fn generate_network<R: RngCore + CryptoRng>(
    root: &Path,
    validators: &[NodeAddr],
    full_nodes: &[NodeAddr],
    chain_id: &str,
    rng: &mut R,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let n = validators.len();
    let all: Vec<&NodeAddr> = validators.iter().chain(full_nodes).collect();
    let validator_secrets: Vec<[u8; 32]> = (0..n).map(|_| keys::generate(rng)).collect();
    let node_secrets: Vec<[u8; 32]> = (0..all.len()).map(|_| keys::generate(rng)).collect();
    let fee_secrets: Vec<[u8; 32]> = (0..n).map(|_| keys::generate(rng)).collect();
    let faucet_secret = keys::generate(rng);

    let fee_asset = fee_asset_for(chain_id);
    let funding = genesis_funding_tx(fee_asset, &vec![(key_path_script(&faucet_secret), FAUCET_OUTPUT_VALUE); FAUCET_OUTPUTS]);
    let genesis = Genesis {
        chain_id: chain_id.to_string(),
        genesis_time: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        validators: validator_secrets
            .iter()
            .map(|s| GenesisValidator { public_key: hex::encode(keys::private_key(*s).public_key().as_bytes()), voting_power: 1 })
            .collect(),
        fee_asset: fee_asset.to_string(),
        initial_txs: vec![hex::encode(elements::encode::serialize(&funding))],
    };

    let peer_ids = node_secrets
        .iter()
        .map(|s| Ok(Keypair::ed25519_from_bytes(*s)?.public().to_peer_id()))
        .collect::<Result<Vec<_>>>()?;
    let peers_of = |i: usize| -> Vec<String> {
        (0..all.len()).filter(|j| *j != i).map(|j| format!("{}/p2p/{}", all[j].dial, peer_ids[j])).collect()
    };

    std::fs::create_dir_all(root)?;
    keys::save(root.join("faucet.json"), &faucet_secret)?;

    let mut homes = (Vec::with_capacity(n), Vec::with_capacity(full_nodes.len()));
    for (i, node) in all.iter().enumerate() {
        let full = i >= n;
        let name = if full { format!("full-{}", i - n) } else { format!("node-{i}") };
        let home = Home(root.join(&name));
        std::fs::create_dir_all(home.0.join("keys"))?;
        genesis.save(home.genesis())?;
        keys::save(home.node_key(), &node_secrets[i])?;
        let fee_payout_script = if full {
            String::new()
        } else {
            keys::save(home.validator_key(), &validator_secrets[i])?;
            keys::save(home.0.join("keys").join("fee.json"), &fee_secrets[i])?;
            hex::encode(key_path_script(&fee_secrets[i]).as_bytes())
        };
        NodeConfig {
            moniker: name,
            role: if full { Role::Full } else { Role::Validator },
            block_time_ms: 1_000,
            fee_payout_script,
            p2p: P2p { listen_addr: node.listen.clone(), persistent_peers: peers_of(i), persistent_peers_only: true },
            rpc: node.rpc.clone().map(|listen_addr| Rpc { listen_addr }),
        }
        .save(home.config())?;
        if full {
            homes.1.push(home.0);
        } else {
            homes.0.push(home.0);
        }
    }
    Ok(homes)
}
