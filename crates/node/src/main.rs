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
    /// Generate a testnet: one home per validator under --home, plus faucet.json.
    Testnet {
        #[arg(long, default_value_t = 4)]
        nodes: usize,
        #[arg(long)]
        home: PathBuf,
        /// P2P port (local: first port; docker: every container).
        #[arg(long, default_value_t = 27000)]
        base_port: u16,
        /// RPC port (local: first port; docker: every container).
        #[arg(long, default_value_t = 18443)]
        rpc_port: u16,
        /// Container host names (comma-separated) instead of 127.0.0.1.
        #[arg(long, value_delimiter = ',')]
        hosts: Option<Vec<String>>,
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
        Cmd::Testnet { nodes, home, base_port, rpc_port, hosts, chain_id } => {
            let addrs = match hosts {
                Some(hosts) => testnet::docker(&hosts, base_port, rpc_port),
                None => testnet::local(nodes, base_port, Some(rpc_port)),
            };
            for dir in testnet::generate(&home, &addrs, &chain_id, &mut rand::thread_rng())? {
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
