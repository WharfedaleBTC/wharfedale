//! M2: four in-process validators decide blocks; a stopped validator restarts and catches up;
//! every node ends with the same chain.

use std::net::TcpListener;
use std::path::PathBuf;
use std::time::Duration;

use rand::rngs::StdRng;
use rand::SeedableRng;
use tokio::sync::watch;
use wharfedale_node::config::NodeConfig;
use wharfedale_node::node::{self, Home, NodeHandle};
use wharfedale_node::testnet;
use wharfedale_store::Store;

const BLOCK_TIME_MS: u64 = 300;

fn free_ports(n: usize) -> Vec<u16> {
    let listeners: Vec<TcpListener> = (0..n).map(|_| TcpListener::bind("127.0.0.1:0").unwrap()).collect();
    listeners.iter().map(|l| l.local_addr().unwrap().port()).collect()
}

fn setup(n: usize) -> (tempfile::TempDir, Vec<PathBuf>) {
    let dir = tempfile::tempdir().unwrap();
    let addrs: Vec<_> = free_ports(n)
        .into_iter()
        .map(|p| testnet::NodeAddr { listen: format!("/ip4/127.0.0.1/tcp/{p}"), dial: format!("/ip4/127.0.0.1/tcp/{p}"), rpc: None })
        .collect();
    let homes = testnet::generate(dir.path(), &addrs, "m2-test", &mut StdRng::seed_from_u64(7)).unwrap();
    for home in &homes {
        let path = Home(home.clone()).config();
        let mut config = NodeConfig::load(&path).unwrap();
        config.block_time_ms = BLOCK_TIME_MS;
        config.save(&path).unwrap();
    }
    (dir, homes)
}

async fn wait_for(tip: &mut watch::Receiver<u64>, height: u64, secs: u64) {
    let reached = tokio::time::timeout(Duration::from_secs(secs), tip.wait_for(|h| *h >= height))
        .await
        .map(|r| r.is_ok());
    assert!(matches!(reached, Ok(true)), "timed out waiting for height {height}, at {}", *tip.borrow());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn four_validators_decide_restart_and_agree() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let (_dir, homes) = setup(4);

    let mut nodes: Vec<Option<NodeHandle>> = Vec::new();
    for home in &homes {
        nodes.push(Some(node::start(home).await.unwrap()));
    }
    for n in nodes.iter_mut().flatten() {
        wait_for(&mut n.tip, 5, 60).await;
    }

    // Stop one validator; 3 of 4 (> 2/3 of voting power) keep deciding.
    nodes[3].take().unwrap().shutdown().await;
    let target = *nodes[0].as_ref().unwrap().tip.borrow() + 3;
    for n in nodes.iter_mut().flatten() {
        wait_for(&mut n.tip, target, 60).await;
    }

    // Restart it: it catches up through value sync and keeps deciding.
    tokio::time::sleep(Duration::from_millis(500)).await;
    nodes[3] = Some(node::start(&homes[3]).await.unwrap());
    let target = *nodes[0].as_ref().unwrap().tip.borrow() + 2;
    for n in nodes.iter_mut().flatten() {
        wait_for(&mut n.tip, target, 90).await;
    }

    for n in nodes.into_iter().flatten() {
        n.shutdown().await;
    }

    // Same block at every height on every node.
    let stores: Vec<Store> = homes.iter().map(|h| Store::open(Home(h.clone()).store()).unwrap()).collect();
    let common = stores.iter().map(|s| s.tip().unwrap().0).min().unwrap();
    assert!(common >= target);
    for height in 0..=common {
        let hashes: Vec<_> = stores.iter().map(|s| s.header(height).unwrap().unwrap().hash()).collect();
        assert!(hashes.windows(2).all(|w| w[0] == w[1]), "fork at height {height}");
        for s in &stores {
            assert!(height == 0 || s.cert(height).unwrap().is_some(), "missing certificate at {height}");
        }
    }
}
