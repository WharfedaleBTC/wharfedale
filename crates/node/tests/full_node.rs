//! Non-validator (full) nodes: follow the chain the validators decide, accept and relay
//! transactions through their RPC, catch up after a restart, and never need a validator key.

use std::net::TcpListener;
use std::time::Duration;

use rand::rngs::StdRng;
use rand::SeedableRng;
use serde_json::{json, Value};
use tokio::sync::watch;
use wharfedale_node::config::{NodeConfig, Role};
use wharfedale_node::node::{self, Home, NodeHandle};
use wharfedale_node::testnet::{self, NodeAddr};
use wharfedale_sdk::{Client, Key, SendOptions, Wallet};

fn free_ports(n: usize) -> Vec<u16> {
    let ls: Vec<TcpListener> = (0..n).map(|_| TcpListener::bind("127.0.0.1:0").unwrap()).collect();
    ls.iter().map(|l| l.local_addr().unwrap().port()).collect()
}

fn addrs(n: usize) -> Vec<NodeAddr> {
    let (p2p, rpc) = (free_ports(n), free_ports(n));
    (0..n)
        .map(|i| NodeAddr {
            listen: format!("/ip4/127.0.0.1/tcp/{}", p2p[i]),
            dial: format!("/ip4/127.0.0.1/tcp/{}", p2p[i]),
            rpc: Some(format!("127.0.0.1:{}", rpc[i])),
        })
        .collect()
}

fn url(a: &NodeAddr) -> String {
    format!("http://{}", a.rpc.as_ref().unwrap())
}

async fn wait_for(tip: &mut watch::Receiver<u64>, height: u64, secs: u64) {
    let ok = tokio::time::timeout(Duration::from_secs(secs), tip.wait_for(|h| *h >= height)).await.map(|r| r.is_ok());
    assert!(matches!(ok, Ok(true)), "timed out waiting for height {height}, at {}", *tip.borrow());
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.unwrap()
}

async fn block_hash(url: String, h: u64) -> Value {
    blocking(move || Client::new(url).call("getblockhash", json!([h])).unwrap()).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn full_node_follows_relays_and_catches_up() {
    let dir = tempfile::tempdir().unwrap();
    let all = addrs(5);
    let (validators, full) = (all[..4].to_vec(), all[4..].to_vec());
    let (vhomes, fhomes) =
        testnet::generate_network(dir.path(), &validators, &full, "full-node-test", &mut StdRng::seed_from_u64(11)).unwrap();
    for home in vhomes.iter().chain(&fhomes) {
        let path = Home(home.clone()).config();
        let mut c = NodeConfig::load(&path).unwrap();
        c.block_time_ms = 300;
        c.save(&path).unwrap();
    }

    // The full node's home has no validator key, and its role says so.
    let fhome = Home(fhomes[0].clone());
    assert!(!fhome.validator_key().exists());
    assert_eq!(NodeConfig::load(fhome.config()).unwrap().role, Role::Full);

    let mut vnodes: Vec<NodeHandle> = Vec::new();
    for h in &vhomes {
        vnodes.push(node::start(h).await.unwrap());
    }
    let mut fnode = node::start(&fhomes[0]).await.unwrap();
    wait_for(&mut fnode.tip, 3, 60).await;

    // A transaction submitted to the full node is gossiped to the validators and confirmed.
    let faucet = Key::load(&dir.path().join("faucet.json")).unwrap();
    let alice = Key::generate();
    let (furl, to) = (url(&full[0]), alice.address().to_string());
    let txid = blocking(move || Wallet::new(Client::new(furl), faucet).send(&to, 1_000_000, &SendOptions::default()).unwrap()).await;
    let (vurl, t) = (url(&validators[2]), txid);
    let height = blocking(move || Client::new(vurl).wait_for_tx(&t, Duration::from_secs(60)).unwrap()).await;
    wait_for(&mut fnode.tip, height, 30).await;
    let (furl, a) = (url(&full[0]), alice.clone());
    let balance = blocking(move || Wallet::new(Client::new(furl), a).balances().unwrap().values().sum::<u64>()).await;
    assert_eq!(balance, 1_000_000);

    // Same chain as the validators.
    assert_eq!(block_hash(url(&full[0]), height).await, block_hash(url(&validators[0]), height).await);

    // Stopped, the full node misses blocks; restarted, it catches up by value sync.
    fnode.shutdown().await;
    let behind = *vnodes[0].tip.borrow() + 4;
    wait_for(&mut vnodes[0].tip, behind, 60).await;
    let mut fnode = node::start(&fhomes[0]).await.unwrap();
    wait_for(&mut fnode.tip, behind, 60).await;
    assert_eq!(block_hash(url(&full[0]), behind).await, block_hash(url(&validators[1]), behind).await);

    // Consensus never waited for the full node: the validators kept deciding without it, and
    // a validator role on its home fails for lack of a key.
    let mut c = NodeConfig::load(fhome.config()).unwrap();
    c.role = Role::Validator;
    c.save(fhome.config()).unwrap();
    fnode.shutdown().await;
    let err = node::start(&fhomes[0]).await.err().expect("no validator key");
    assert!(format!("{err:#}").contains("validator.json"), "{err:#}");

    for n in vnodes {
        n.shutdown().await;
    }
}
