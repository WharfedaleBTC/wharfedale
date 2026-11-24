//! `wharfedale-wallet`: single-key wallet over the node's JSON-RPC and Esplora REST, a thin
//! CLI over the Rust SDK (`wharfedale-sdk`). Spec: `docs/src/wallet.md`.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use clap::{Parser, Subcommand};
use eyre::Result;
use serde_json::{json, Value};
use wharfedale_sdk::contract::{self, ContractSpend};
use wharfedale_sdk::elements::AssetId;
use wharfedale_sdk::{Client, Contract, Key, SendOptions, Wallet};

/// A SimplicityHL program and its arguments.
#[derive(clap::Args)]
struct ProgramArgs {
    /// SimplicityHL source file (`.simf`).
    #[arg(long)]
    program: PathBuf,
    /// Arguments: JSON object of `param::` values (default `{}`).
    #[arg(long)]
    args: Option<PathBuf>,
}

impl ProgramArgs {
    fn load(&self) -> Result<Contract> {
        let src = std::fs::read_to_string(&self.program)?;
        let args = match &self.args {
            Some(p) => std::fs::read_to_string(p)?,
            None => "{}".into(),
        };
        Ok(Contract::compile(&src, &args)?)
    }
}

#[derive(Parser)]
#[command(name = "wharfedale-wallet", about = "Wharfedale single-key wallet")]
struct Cli {
    /// Node JSON-RPC URL.
    #[arg(long, global = true, default_value = "http://127.0.0.1:18443")]
    rpc: String,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a key file and print its confidential address.
    Keygen { #[arg(long)] out: PathBuf },
    /// Print the confidential address `wfc1…` of a key (`--explicit`: `wf1…`; `--script`: hex).
    Address {
        #[arg(long)]
        key: PathBuf,
        #[arg(long)]
        explicit: bool,
        #[arg(long)]
        script: bool,
    },
    /// Confirmed balance per asset (unspent outputs not spent in the mempool).
    Balance {
        #[arg(long)]
        key: PathBuf,
        /// Print only this asset's amount.
        #[arg(long)]
        asset: Option<String>,
    },
    /// Send `amount` of `asset` (default: the fee asset) to an address or hex script.
    Send {
        #[arg(long)]
        key: PathBuf,
        #[arg(long)]
        to: String,
        #[arg(long)]
        amount: u64,
        #[arg(long)]
        asset: Option<String>,
        /// Fee in fee-asset units.
        #[arg(long, default_value_t = 1_000)]
        fee: u64,
    },
    /// Issue a new asset (explicit amounts), paid confidentially to this key.
    /// Prints txid, asset id and reissuance-token id.
    Issue {
        #[arg(long)]
        key: PathBuf,
        #[arg(long)]
        amount: u64,
        /// Reissuance tokens to create (0 = fixed supply).
        #[arg(long, default_value_t = 1)]
        tokens: u64,
        #[arg(long, default_value_t = 1_000)]
        fee: u64,
    },
    /// Reissue more of an asset, spending (and keeping) its reissuance token.
    Reissue {
        #[arg(long)]
        key: PathBuf,
        #[arg(long)]
        asset: String,
        #[arg(long)]
        amount: u64,
        #[arg(long, default_value_t = 1_000)]
        fee: u64,
    },
    /// Print a key's x-only public key (untweaked), for contract parameters.
    Pubkey { #[arg(long)] key: PathBuf },
    /// Print a signature-only contract's artifact (CMR, program, signature order) as JSON, for
    /// clients without a SimplicityHL compiler (TypeScript SDK).
    ContractArtifact {
        #[command(flatten)]
        program: ProgramArgs,
    },
    /// Print a contract's address `wf1p…` (`--script`: output script hex; `--cmr`: CMR hex).
    ContractAddress {
        #[command(flatten)]
        program: ProgramArgs,
        #[arg(long)]
        script: bool,
        #[arg(long)]
        cmr: bool,
    },
    /// Confirmed balance per asset locked to a contract.
    ContractBalance {
        #[command(flatten)]
        program: ProgramArgs,
        #[arg(long)]
        asset: Option<String>,
    },
    /// Spend contract coins: `amount` to `to`, the fee and change from/to the contract.
    ContractSpend {
        #[command(flatten)]
        program: ProgramArgs,
        /// Witness: JSON object of `witness::` values; `"sign:<key file>"` signs `sig_all_hash`.
        #[arg(long)]
        witness: Option<PathBuf>,
        #[arg(long)]
        to: String,
        #[arg(long)]
        amount: u64,
        #[arg(long)]
        asset: Option<String>,
        #[arg(long, default_value_t = 1_000)]
        fee: u64,
        /// Transaction lock time (block height), for contracts that check it.
        #[arg(long)]
        lock_time: Option<u32>,
    },
    /// Wait until a transaction has a confirmation; print its block height.
    WaitTx { #[arg(long)] txid: String, #[arg(long, default_value_t = 60)] timeout: u64 },
    /// Wait until the chain reaches a height.
    WaitHeight { #[arg(long)] height: u64, #[arg(long, default_value_t = 60)] timeout: u64 },
    /// Call any RPC method with JSON params, e.g. `rpc getblockchaininfo`.
    Rpc { method: String, params: Vec<String> },
    /// GET an Esplora REST path, e.g. `rest asset/<id>`; prints JSON.
    Rest { path: String },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = Client::new(cli.rpc);
    let wallet = |key: &Path| -> Result<Wallet> { Ok(Wallet::new(client.clone(), Key::load(key)?)) };
    let asset = |a: Option<String>| -> Result<Option<AssetId>> { Ok(a.as_deref().map(AssetId::from_str).transpose()?) };
    match cli.cmd {
        Cmd::Keygen { out } => {
            let key = Key::generate();
            key.save(&out)?;
            println!("{}", key.address());
        }
        Cmd::Address { key, explicit, script } => {
            let key = Key::load(&key)?;
            if script {
                println!("{}", hex::encode(key.script_pubkey().as_bytes()));
            } else if explicit {
                println!("{}", key.explicit_address());
            } else {
                println!("{}", key.address());
            }
        }
        Cmd::Balance { key, asset } => print_balances(&wallet(&key)?.balances()?, asset)?,
        Cmd::Send { key, to, amount, asset: a, fee } => {
            println!("{}", wallet(&key)?.send(&to, amount, &SendOptions { asset: asset(a)?, fee })?);
        }
        Cmd::Issue { key, amount, tokens, fee } => {
            let issued = wallet(&key)?.issue(amount, tokens, fee)?;
            println!("{}\n{}\n{}", issued.txid, issued.asset, issued.token);
        }
        Cmd::Reissue { key, asset: a, amount, fee } => {
            println!("{}", wallet(&key)?.reissue(AssetId::from_str(&a)?, amount, fee)?);
        }
        Cmd::Pubkey { key } => println!("{}", Key::load(&key)?.xonly_public_key()),
        Cmd::ContractArtifact { program } => {
            println!("{}", serde_json::to_string_pretty(&program.load()?.signature_artifact()?)?);
        }
        Cmd::ContractAddress { program, script, cmr } => {
            let c = program.load()?;
            if script {
                println!("{}", hex::encode(c.script_pubkey().as_bytes()));
            } else if cmr {
                println!("{}", hex::encode(c.cmr()));
            } else {
                println!("{}", c.address());
            }
        }
        Cmd::ContractBalance { program, asset } => {
            let mut map = BTreeMap::<AssetId, u64>::new();
            for coin in contract::coins(&client, &program.load()?)? {
                *map.entry(coin.asset()).or_insert(0) += coin.value();
            }
            print_balances(&map, asset)?;
        }
        Cmd::ContractSpend { program, witness, to, amount, asset: a, fee, lock_time } => {
            let witness: Value = match witness {
                Some(p) => serde_json::from_slice(&std::fs::read(&p)?)?,
                None => json!({}),
            };
            let spend = ContractSpend { to, amount, asset: asset(a)?, fee, lock_time, witness };
            // "sign:<key file>": the witness names key files.
            let signer = |path: &str| Key::load(Path::new(path));
            println!("{}", contract::spend(&client, &program.load()?, &spend, &signer)?);
        }
        Cmd::WaitTx { txid, timeout } => {
            println!("{}", client.wait_for_tx(&txid.parse()?, Duration::from_secs(timeout))?);
        }
        Cmd::WaitHeight { height, timeout } => {
            println!("{}", client.wait_for_height(height, Duration::from_secs(timeout))?);
        }
        Cmd::Rest { path } => {
            println!("{}", serde_json::to_string_pretty(&client.rest(&path)?)?);
        }
        Cmd::Rpc { method, params } => {
            let params: Vec<Value> =
                params.iter().map(|p| serde_json::from_str(p).unwrap_or_else(|_| Value::String(p.clone()))).collect();
            println!("{}", serde_json::to_string_pretty(&client.call(&method, Value::Array(params))?)?);
        }
    }
    Ok(())
}

fn print_balances(balances: &BTreeMap<AssetId, u64>, asset: Option<String>) -> Result<()> {
    match asset {
        Some(a) => println!("{}", balances.get(&AssetId::from_str(&a)?).copied().unwrap_or(0)),
        None => {
            for (asset, amount) in balances {
                println!("{asset} {amount}");
            }
        }
    }
    Ok(())
}
