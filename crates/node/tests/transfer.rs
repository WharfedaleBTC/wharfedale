//! End to end, in process: 4 validators with RPC and Esplora.
//! M3: faucet → alice via node 0, alice → bob via node 2; fee payouts add up.
//! M4: the transfers are confidential (wfc1 addresses); alice issues and reissues an asset;
//! the Esplora API reports blocks, transactions, outspends, address UTXOs and the asset.
//! M5: coins locked to a SimplicityHL p2pk contract are spent with the owner's signature.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rand::rngs::StdRng;
use rand::SeedableRng;
use serde_json::{json, Value};
use wharfedale_node::config::NodeConfig;
use wharfedale_node::node::{self, Home, NodeHandle};
use wharfedale_node::testnet::{self, FAUCET_OUTPUT_VALUE};
use wharfedale_sdk::Contract;
use sdk::*;

/// The flows below through the Rust SDK (`wharfedale-sdk`), with keys as raw secrets.
mod sdk {
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::str::FromStr;

    use serde_json::Value;
    use wharfedale_sdk::elements::{Address, AssetId, Txid};
    use wharfedale_sdk::{Client, Key, Result, SendOptions, Wallet};

    pub struct Rpc(pub String);

    impl Rpc {
        pub fn call(&self, method: &str, params: Value) -> Result<Value> {
            Client::new(&self.0).call(method, params)
        }
        pub fn rest(&self, path: &str) -> Result<Value> {
            Client::new(&self.0).rest(path)
        }
    }

    fn wallet(rpc: &Rpc, secret: &[u8; 32]) -> Wallet {
        Wallet::new(Client::new(&rpc.0), Key::from_secret(*secret).unwrap())
    }

    pub fn load_key(path: &Path) -> Result<[u8; 32]> {
        Ok(Key::load(path)?.secret_bytes())
    }
    pub fn address(secret: &[u8; 32]) -> Address {
        Key::from_secret(*secret).unwrap().address()
    }
    pub fn explicit_address(secret: &[u8; 32]) -> Address {
        Key::from_secret(*secret).unwrap().explicit_address()
    }
    pub fn send(rpc: &Rpc, secret: &[u8; 32], to: &str, amount: u64, asset: Option<&str>, fee: u64) -> Result<Txid> {
        let asset = asset.map(|a| AssetId::from_str(a).unwrap());
        wallet(rpc, secret).send(to, amount, &SendOptions { asset, fee })
    }
    pub fn balances(rpc: &Rpc, secret: &[u8; 32]) -> Result<BTreeMap<AssetId, u64>> {
        wallet(rpc, secret).balances()
    }
    pub fn issue(rpc: &Rpc, secret: &[u8; 32], amount: u64, tokens: u64, fee: u64) -> Result<(Txid, AssetId, AssetId)> {
        let i = wallet(rpc, secret).issue(amount, tokens, fee)?;
        Ok((i.txid, i.asset, i.token))
    }
    pub fn reissue(rpc: &Rpc, secret: &[u8; 32], asset: &str, amount: u64, fee: u64) -> Result<Txid> {
        wallet(rpc, secret).reissue(AssetId::from_str(asset).unwrap(), amount, fee)
    }

    pub mod contract {
        use std::path::Path;

        use serde_json::Value;
        use wharfedale_sdk::contract::ContractSpend;
        use wharfedale_sdk::elements::Txid;
        use wharfedale_sdk::{Client, Coin, Contract, Key, Result};

        use super::Rpc;

        pub fn pubkey(secret: &[u8; 32]) -> String {
            Key::from_secret(*secret).unwrap().xonly_public_key().to_string()
        }
        pub fn coins(rpc: &Rpc, c: &Contract) -> Result<Vec<Coin>> {
            wharfedale_sdk::contract::coins(&Client::new(&rpc.0), c)
        }
        /// `"sign:<key file>"` witnesses.
        #[allow(clippy::too_many_arguments)]
        pub fn spend(rpc: &Rpc, c: &Contract, witness: &Value, to: &str, amount: u64, _asset: Option<&str>, fee: u64, lock_time: Option<u32>) -> Result<Txid> {
            let spend = ContractSpend { to: to.into(), amount, asset: None, fee, lock_time, witness: witness.clone() };
            wharfedale_sdk::contract::spend(&Client::new(&rpc.0), c, &spend, &|p: &str| Key::load(Path::new(p)))
        }
    }
}

const FEE: u64 = 1_000;
const P2PK: &str = "fn main() {
    jet::bip_0340_verify((param::OWNER, jet::sig_all_hash()), witness::SIG)
}";

fn p2pk(owner: &[u8; 32]) -> Contract {
    Contract::compile(P2PK, &format!(r#"{{"OWNER": "0x{}"}}"#, contract::pubkey(owner))).unwrap()
}

fn free_ports(n: usize) -> Vec<u16> {
    let ls: Vec<TcpListener> = (0..n).map(|_| TcpListener::bind("127.0.0.1:0").unwrap()).collect();
    ls.iter().map(|l| l.local_addr().unwrap().port()).collect()
}

fn write_key(path: &Path, seed: u8) -> [u8; 32] {
    let secret = [seed; 32];
    std::fs::write(path, json!({ "secret": hex::encode(secret) }).to_string()).unwrap();
    secret
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.unwrap()
}

/// Polls `f` every 250 ms until it returns `Some`, for up to 60 s.
async fn eventually<T: Send + 'static>(what: &str, f: impl Fn() -> Option<T> + Send + Sync + Clone + 'static) -> T {
    for _ in 0..240 {
        let g = f.clone();
        if let Some(v) = blocking(g).await {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("timed out: {what}");
}

async fn confirmed(url: String, txid: String) -> u64 {
    eventually("confirmation", move || {
        Rpc(url.clone()).call("getrawtransaction", json!([txid.clone(), true])).ok()?.get("blockheight")?.as_u64()
    })
    .await
}

fn rest(url: &str, path: &str) -> Option<Value> {
    Rpc(url.to_string()).rest(path).ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn confidential_transfers_issuance_and_esplora() {
    let dir = tempfile::tempdir().unwrap();
    let p2p = free_ports(4);
    let rpc = free_ports(4);
    let addrs: Vec<_> = (0..4)
        .map(|i| testnet::NodeAddr {
            listen: format!("/ip4/127.0.0.1/tcp/{}", p2p[i]),
            dial: format!("/ip4/127.0.0.1/tcp/{}", p2p[i]),
            rpc: Some(format!("127.0.0.1:{}", rpc[i])),
        })
        .collect();
    let homes: Vec<PathBuf> = testnet::generate(dir.path(), &addrs, "m4-test", &mut StdRng::seed_from_u64(3)).unwrap();
    for home in &homes {
        let path = Home(home.clone()).config();
        let mut c = NodeConfig::load(&path).unwrap();
        c.block_time_ms = 300;
        c.save(&path).unwrap();
    }
    let urls: Vec<String> = rpc.iter().map(|p| format!("http://127.0.0.1:{p}")).collect();

    let mut nodes: Vec<NodeHandle> = Vec::new();
    for home in &homes {
        nodes.push(node::start(home).await.unwrap());
    }
    // No empty blocks: with nothing to include, the chain stays at genesis.
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert!(nodes.iter().all(|n| *n.tip.borrow() == 0), "an idle network built a block");

    let faucet = load_key(&dir.path().join("faucet.json")).unwrap();
    let alice = write_key(&dir.path().join("alice.json"), 0xa1);
    let bob = write_key(&dir.path().join("bob.json"), 0xb0);
    let (alice_addr, bob_addr) = (address(&alice).to_string(), address(&bob).to_string());
    assert!(alice_addr.starts_with("wfc1p"), "{alice_addr}");

    // faucet (explicit coins) → alice (confidential), via node 0, seen by node 3.
    let u = urls[0].clone();
    let fund = blocking(move || send(&Rpc(u), &faucet, &alice_addr, 5_000_000, None, FEE)).await.unwrap();
    confirmed(urls[3].clone(), fund.to_string()).await;

    // alice (confidential coin) → bob (confidential), via node 2.
    let u = urls[2].clone();
    let pay = blocking(move || send(&Rpc(u), &alice, &bob_addr, 1_000_000, None, FEE)).await.unwrap();
    confirmed(urls[1].clone(), pay.to_string()).await;

    let info = blocking({ let u = urls[0].clone(); move || Rpc(u).call("getblockchaininfo", json!([])).unwrap() }).await;
    let fee_asset: elements::AssetId = info["fee_asset"].as_str().unwrap().parse().unwrap();
    assert_eq!(info["role"], json!("validator"));
    let balance = |secret: [u8; 32], asset: elements::AssetId| {
        let u = urls[1].clone();
        blocking(move || balances(&Rpc(u), &secret).unwrap().get(&asset).copied().unwrap_or(0))
    };
    assert_eq!(balance(alice, fee_asset).await, 5_000_000 - 1_000_000 - FEE);
    assert_eq!(balance(bob, fee_asset).await, 1_000_000);
    assert_eq!(balance(faucet, fee_asset).await, testnet::FAUCET_OUTPUTS as u64 * FAUCET_OUTPUT_VALUE - 5_000_000 - FEE);

    // Confidential on chain: the payment output shows commitments, not amounts.
    let tx = rest(&urls[0], &format!("tx/{pay}")).unwrap();
    assert!(tx["vout"][0]["valuecommitment"].is_string() && tx["vout"][0]["value"].is_null(), "{tx}");
    assert_eq!(tx["status"]["confirmed"], json!(true));

    // Coinbases claimed both fees.
    let mut claimed = 0;
    for home in &homes {
        claimed += balance(load_key(&home.join("keys").join("fee.json")).unwrap(), fee_asset).await;
    }
    assert_eq!(claimed, 2 * FEE);

    // Issue 1 000 units + 1 token, then reissue 500 more with the token.
    let u = urls[0].clone();
    let (iss_txid, asset, token) = blocking(move || issue(&Rpc(u), &alice, 1_000, 1, FEE)).await.unwrap();
    let iss_height = confirmed(urls[0].clone(), iss_txid.to_string()).await;
    assert_eq!(balance(alice, asset).await, 1_000);
    assert_eq!(balance(alice, token).await, 1);

    let (u, a) = (urls[2].clone(), asset.to_string());
    eventually("asset indexed on node 2", move || rest(&u, &format!("asset/{a}"))).await;
    let (u, a) = (urls[2].clone(), asset.to_string());
    let re_txid = blocking(move || reissue(&Rpc(u), &alice, &a, 500, FEE)).await.unwrap();
    confirmed(urls[0].clone(), re_txid.to_string()).await;
    assert_eq!(balance(alice, asset).await, 1_500);
    assert_eq!(balance(alice, token).await, 1);

    let (u, a) = (urls[1].clone(), asset.to_string());
    let record = eventually("reissuance indexed", move || {
        rest(&u, &format!("asset/{a}")).filter(|r| r["chain_stats"]["issuance_count"] == json!(2))
    })
    .await;
    assert_eq!(record["chain_stats"]["issued_amount"], json!(1_500));
    assert_eq!(record["reissuance_token"], json!(token.to_string()));

    // Esplora views.
    let u = urls[3].clone();
    let height: u64 = blocking(move || ureq::get(&format!("{u}/blocks/tip/height")).call().unwrap().into_string().unwrap()).await.parse().unwrap();
    assert!(height >= 4);
    let blocks = rest(&urls[3], "blocks").unwrap();
    let heights: Vec<u64> = blocks.as_array().unwrap().iter().map(|b| b["height"].as_u64().unwrap()).collect();
    assert!(heights.len() == 10.min(heights[0] as usize + 1) && heights[0] >= height && heights.windows(2).all(|w| w[0] == w[1] + 1), "{heights:?}");
    let page = rest(&urls[3], &format!("blocks/{iss_height}")).unwrap();
    assert_eq!(page[0]["height"], json!(iss_height));
    let iss_block = page[0]["id"].as_str().unwrap().to_string();
    assert_eq!(page[0], rest(&urls[3], &format!("block/{iss_block}")).unwrap());
    let txs = rest(&urls[3], &format!("block/{iss_block}/txs")).unwrap();
    let txs = txs.as_array().unwrap();
    assert_eq!(txs.len() as u64, page[0]["tx_count"].as_u64().unwrap());
    assert_eq!(txs[0]["vin"][0]["is_coinbase"], json!(true));
    let iss_json = txs.iter().find(|t| t["txid"] == json!(iss_txid.to_string())).expect("issuance in its block");
    assert_eq!(iss_json, &rest(&urls[3], &format!("tx/{iss_txid}")).unwrap());
    assert_eq!(rest(&urls[3], &format!("block/{iss_block}/txs/1")).unwrap().as_array().unwrap().len(), txs.len() - 1);
    // The wallet's issuance carries no WFA1 output: no name.
    assert!(record["name"].is_null() && record["ticker"].is_null() && record["precision"].is_null(), "{record}");
    let (u, f) = (urls[3].clone(), fund.to_string());
    let outspends = eventually("outspends indexed", move || rest(&u, &format!("tx/{f}/outspends")).filter(|v| v[0]["spent"] == json!(true))).await;
    assert_eq!(outspends[0]["txid"], json!(pay.to_string()));
    let alice_explicit = explicit_address(&alice).to_string();
    let utxo = rest(&urls[3], &format!("address/{alice_explicit}/utxo")).unwrap();
    let utxo = utxo.as_array().unwrap();
    assert!(!utxo.is_empty() && utxo.iter().all(|u| u["valuecommitment"].is_string() && u["range_proof"].is_string()));
    let history = rest(&urls[3], &format!("address/{alice_explicit}/txs")).unwrap();
    assert!(history.as_array().unwrap().len() >= 3, "{history}");

    // M5: faucet locks 300 000 to bob's p2pk contract; only bob's signature spends it.
    let caddr = p2pk(&bob).address().to_string();
    assert!(caddr.starts_with("wf1p"), "{caddr}");
    let u = urls[1].clone();
    let lock = blocking(move || send(&Rpc(u), &faucet, &caddr, 300_000, None, FEE)).await.unwrap();
    confirmed(urls[2].clone(), lock.to_string()).await;
    let contract_balance = |owner: [u8; 32]| {
        let u = urls[0].clone();
        blocking(move || contract::coins(&Rpc(u), &p2pk(&owner)).unwrap().iter().map(|c| c.value()).sum::<u64>())
    };
    assert_eq!(contract_balance(bob).await, 300_000);

    let sign_with = |name: &str| json!({ "SIG": format!("sign:{}", dir.path().join(name).display()) });
    let (u, w, to) = (urls[3].clone(), sign_with("alice.json"), address(&alice).to_string());
    let err = blocking(move || contract::spend(&Rpc(u), &p2pk(&bob), &w, &to, 200_000, None, FEE, None)).await.unwrap_err();
    assert!(err.to_string().contains("not satisfied"), "{err}");

    let alice_before = balance(alice, fee_asset).await;
    let (u, w, to) = (urls[3].clone(), sign_with("bob.json"), address(&alice).to_string());
    let spend = blocking(move || contract::spend(&Rpc(u), &p2pk(&bob), &w, &to, 200_000, None, FEE, None)).await.unwrap();
    confirmed(urls[0].clone(), spend.to_string()).await;
    assert_eq!(contract_balance(bob).await, 300_000 - 200_000 - FEE);
    assert_eq!(balance(alice, fee_asset).await, alice_before + 200_000);
    let (u, ca) = (urls[2].clone(), p2pk(&bob).address().to_string());
    let history = eventually("contract history indexed", move || {
        rest(&u, &format!("address/{ca}/txs")).filter(|h| h.as_array().is_some_and(|a| a.len() == 2))
    })
    .await;
    assert!(history.as_array().unwrap().iter().any(|t| t["txid"] == json!(spend.to_string())));

    // Overspend refused.
    let (u, b) = (urls[0].clone(), address(&bob).to_string());
    assert!(blocking(move || send(&Rpc(u), &alice, &b, 100_000_000, None, FEE)).await.is_err());

    // Idle again: no new blocks. Every block has a transaction, and a coinbase that claimed
    // exactly its fees (every standard transaction pays one).
    let tip = *nodes[0].tip.borrow();
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert!(nodes.iter().all(|n| *n.tip.borrow() == tip), "an idle network built a block");
    for h in 1..=tip {
        let hash = blocking({ let u = urls[0].clone(); move || Rpc(u).call("getblockhash", json!([h])).unwrap() }).await;
        let txs = rest(&urls[0], &format!("block/{}/txs", hash.as_str().unwrap())).unwrap();
        let txs = txs.as_array().unwrap();
        assert!(txs.len() >= 2, "block {h}: {txs:?}");
        assert_eq!(txs[0]["vin"][0]["is_coinbase"], json!(true), "block {h}");
        let fees: u64 = txs[1..].iter().map(|t| t["fee"].as_u64().unwrap()).sum();
        let claimed: u64 = txs[0]["vout"].as_array().unwrap().iter().map(|o| o["value"].as_u64().unwrap()).sum();
        assert_eq!(claimed, fees, "block {h}");
    }

    for n in nodes {
        n.shutdown().await;
    }
}

/// The SDK duplicates protocol constants (it has no node dependencies): they must agree.
#[test]
fn sdk_constants_match_the_node() {
    use wharfedale_sdk::elements::secp256k1_zkp::{Keypair, SECP256K1};
    assert_eq!(wharfedale_sdk::network::ADDRESS_PARAMS, wharfedale_chain::ADDRESS_PARAMS);
    for id in ["wharfedale-docker", "m4-test"] {
        assert_eq!(wharfedale_sdk::network::fee_asset_for(id), wharfedale_chain::fee_asset_for(id));
    }
    let secret = [0x42; 32];
    let key = wharfedale_sdk::Key::from_secret(secret).unwrap();
    assert_eq!(key.script_pubkey(), wharfedale_chain::key_path_script(&secret));
    let cmr = [7u8; 32];
    assert_eq!(wharfedale_sdk::taproot::script_pubkey(&cmr), wharfedale_ledger_simplicity_sys::taproot::script_pubkey(&cmr));
    let _ = Keypair::from_seckey_slice(SECP256K1, &secret).unwrap();
}
