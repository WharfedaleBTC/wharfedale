//! `wharfedale` CLI.
#![forbid(unsafe_code)]

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use eyre::Result;
use tracing_subscriber::EnvFilter;
use wharfedale_node::{node, testnet};

#[derive(Parser)]
#[command(name = "wharfedale", about = "Wharfedale node")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Generate a testnet under --home: validator homes (node-<i>), full-node homes (full-<i>),
    /// faucet.json.
    Testnet {
        /// Validators (local mode).
        #[arg(long, default_value_t = 4)]
        nodes: usize,
        /// Full nodes (local mode): no validator key; follow the chain and serve RPC.
        #[arg(long, default_value_t = 0)]
        full_nodes: usize,
        #[arg(long)]
        home: PathBuf,
        /// P2P port (local: first port; docker: every container).
        #[arg(long, default_value_t = 27000)]
        base_port: u16,
        /// RPC port (local: first port; docker: every container).
        #[arg(long, default_value_t = 18443)]
        rpc_port: u16,
        /// Validator container host names (comma-separated) instead of 127.0.0.1.
        #[arg(long, value_delimiter = ',')]
        hosts: Option<Vec<String>>,
        /// Full-node container host names (comma-separated), with --hosts.
        #[arg(long, value_delimiter = ',')]
        full_hosts: Option<Vec<String>>,
        #[arg(long, default_value = "wharfedale-local")]
        chain_id: String,
    },
    /// Start a node from its home directory.
    Start {
        #[arg(long)]
        home: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    match Cli::parse().cmd {
        Cmd::Testnet { nodes, full_nodes, home, base_port, rpc_port, hosts, full_hosts, chain_id } => {
            let (validators, full) = match hosts {
                Some(hosts) => (
                    testnet::docker(&hosts, base_port, rpc_port),
                    testnet::docker(&full_hosts.unwrap_or_default(), base_port, rpc_port),
                ),
                None => {
                    let all = testnet::local(nodes + full_nodes, base_port, Some(rpc_port));
                    (all[..nodes].to_vec(), all[nodes..].to_vec())
                }
            };
            let (v, f) = testnet::generate_network(&home, &validators, &full, &chain_id, &mut rand::thread_rng())?;
            for dir in v.iter().chain(&f) {
                println!("{}", dir.display());
            }
        }
        Cmd::Start { home } => {
            let handle = node::start(&home).await?;
            tokio::signal::ctrl_c().await?;
            handle.shutdown().await;
        }
    }
    Ok(())
}
