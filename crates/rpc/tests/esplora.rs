//! In process, without consensus: blocks committed straight to a store, served by `serve`.
//! `/blocks` and `/block/:hash/txs` paging, WFA1 asset names on `/asset/:id`, `role` and
//! `getnetworkinfo`.

use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use elements::confidential::{Asset, Nonce, Value as CValue};
use elements::hashes::sha256;
use elements::script::Builder;
use elements::{opcodes, AssetIssuance, OutPoint, Script, Transaction, TxIn, TxOut, TxOutWitness, Txid};
use serde_json::{json, Value};
use wharfedale_chain::{merkle, Block, Genesis, GenesisValidator, Header};
use wharfedale_ledger::{BlockDiff, VerifyCache};
use wharfedale_mempool::{Mempool, Policy};
use wharfedale_rpc::{NetworkInfo, NetworkInfoFn, NodeRole, NodeState, PeerInfo};
use wharfedale_store::Store;

const CHAIN: &str = "rpc-test";
const META: &[u8] = br#"{"name":"Gold","ticker":"GLD","precision":2}"#;

fn out(script: Script) -> TxOut {
    TxOut {
        asset: Asset::Explicit(wharfedale_chain::fee_asset_for(CHAIN)),
        value: CValue::Explicit(0),
        nonce: Nonce::Null,
        script_pubkey: script,
        witness: TxOutWitness::default(),
    }
}

fn taproot() -> Script {
    Script::from([0x51, 0x20].into_iter().chain([3u8; 32]).collect::<Vec<u8>>())
}

/// A distinct transaction spending `(height ‖ n, 0)`; a new issuance when `contract` is set.
fn tx(height: u64, n: u8, contract: Option<[u8; 32]>, extra: Option<TxOut>) -> Transaction {
    let mut prev = [n; 32];
    prev[..8].copy_from_slice(&height.to_le_bytes());
    let mut input = TxIn { previous_output: OutPoint::new(Txid::from_byte_array(prev), 0), ..Default::default() };
    if let Some(c) = contract {
        input.asset_issuance = AssetIssuance {
            asset_blinding_nonce: elements::AssetBlindingNonce::NEW_ISSUANCE,
            asset_entropy: elements::AssetEntropy::from_byte_array(c),
            amount: CValue::Explicit(1_000),
            inflation_keys: CValue::Explicit(1),
        };
    }
    let output = std::iter::once(out(taproot())).chain(extra).collect();
    Transaction { version: 2, lock_time: elements::LockTime::ZERO, input: vec![input], output }
}

fn commit(store: &Store, txs: Vec<Transaction>) -> Block {
    let (tip, _) = store.tip().unwrap();
    let parent = store.header(tip).unwrap().unwrap();
    let block = Block {
        header: Header {
            version: Header::VERSION,
            height: tip + 1,
            prev_block_hash: parent.hash().to_byte_array(),
            timestamp: parent.timestamp + 1,
            proposer: [0; 20],
            tx_merkle_root: merkle::tx_root(&txs),
            witness_merkle_root: merkle::witness_root(&txs),
        },
        txs,
    };
    store.commit(&block, b"cert", &BlockDiff::default()).unwrap();
    block
}

fn state(store: Arc<Store>, network_info: Option<NetworkInfoFn>) -> NodeState {
    let fee_asset = wharfedale_chain::fee_asset_for(CHAIN);
    NodeState {
        genesis_hash: store.genesis_hash().unwrap(),
        store,
        mempool: Arc::new(Mutex::new(Mempool::new(Policy::new(fee_asset)))),
        cache: Arc::new(VerifyCache::new()),
        chain_id: CHAIN.into(),
        fee_asset,
        coinbase_maturity: 100,
        relay: tokio::sync::mpsc::channel(1).0,
        role: NodeRole::Full,
        network_info,
    }
}

/// Serves `st` on a free port; returns its base URL and the shutdown trigger.
fn serve(st: NodeState) -> (String, tokio::sync::oneshot::Sender<()>) {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let addr = format!("127.0.0.1:{port}").parse().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(wharfedale_rpc::serve(addr, st, async move {
        let _ = stopped.await;
    }));
    (format!("http://127.0.0.1:{port}"), stop)
}

/// `GET` → (status, JSON body or the text as a JSON string).
fn get(url: &str) -> (u16, Value) {
    let resp = match ureq::get(url).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(e) => panic!("{url}: {e}"),
    };
    let status = resp.status();
    let text = resp.into_string().unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

fn ok(url: &str) -> Value {
    let (status, v) = get(url);
    assert_eq!(status, 200, "{url}: {v}");
    v
}

fn rpc(url: &str, method: &str) -> Value {
    ureq::post(url).send_json(json!({ "method": method, "params": [], "id": 1 })).unwrap().into_json().unwrap()
}

async fn wait_up(url: &str) {
    for _ in 0..100 {
        let u = format!("{url}/blocks/tip/height");
        if tokio::task::spawn_blocking(move || ureq::get(&u).call().is_ok()).await.unwrap() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("server did not start");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn blocks_block_txs_asset_names_and_network_info() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("store.redb")).unwrap();
    store
        .init_or_check(&Genesis {
            chain_id: CHAIN.into(),
            genesis_time: 1_800_000_000,
            validators: vec![GenesisValidator { public_key: hex::encode([1u8; 32]), voting_power: 1 }],
            fee_asset: wharfedale_chain::fee_asset_for(CHAIN).to_string(),
            initial_txs: vec![],
        })
        .unwrap();

    // Heights 1..=12; block 5 holds 30 transactions, the first two issuances (one named).
    let contract: [u8; 32] = sha256::Hash::hash(META).to_byte_array();
    let wfa1 = Builder::new().push_opcode(opcodes::all::OP_RETURN).push_slice(b"WFA1").push_slice(META).into_script();
    let named = tx(5, 0, Some(contract), Some(out(wfa1)));
    let unnamed = tx(5, 1, Some(contract), None);
    let mut blocks = vec![store.block(0).unwrap().unwrap()];
    for h in 1..=12u64 {
        let txs = if h == 5 {
            [named.clone(), unnamed.clone()].into_iter().chain((2..30).map(|n| tx(h, n, None, None))).collect()
        } else {
            vec![tx(h, 0, None, None)]
        };
        blocks.push(commit(&store, txs));
    }
    while store.index_next().unwrap().is_some() {}
    let store = Arc::new(store);

    let peer = |moniker: &str, validator: bool| PeerInfo {
        peer_id: format!("12D3Koo{moniker}"),
        moniker: moniker.into(),
        address: "/ip4/127.0.0.1/tcp/1".into(),
        validator,
        consensus_address: validator.then(|| "ab".repeat(20)),
        direction: Some("outbound".into()),
        score: 1.5,
    };
    let peers = vec![peer("node-2", false), peer("node-0", true)];
    let net: NetworkInfoFn = Arc::new(move || {
        let peers = peers.clone();
        Box::pin(async move {
            Ok(NetworkInfo {
                peer_id: "12D3KooSelf".into(),
                moniker: "me".into(),
                consensus_address: None,
                listen_addrs: vec!["/ip4/0.0.0.0/tcp/27000".into()],
                peers,
            })
        })
    });
    let (url, stop) = serve(state(store.clone(), Some(net)));
    let (bare_url, bare_stop) = serve(state(store.clone(), None));
    wait_up(&url).await;
    wait_up(&bare_url).await;

    let hash = |h: usize| blocks[h].hash().to_string();
    let (u, h5, h12) = (url.clone(), hash(5), hash(12));
    let checks = tokio::task::spawn_blocking(move || {
        let heights = |v: &Value| v.as_array().unwrap().iter().map(|b| b["height"].as_u64().unwrap()).collect::<Vec<_>>();

        // /blocks: 10 from the tip, each as /block/:hash.
        let page = ok(&format!("{u}/blocks"));
        assert_eq!(heights(&page), (3..=12).rev().collect::<Vec<_>>());
        assert_eq!(page[0], ok(&format!("{u}/block/{h12}")));
        assert_eq!(page[7]["tx_count"], json!(30));
        assert_eq!(heights(&ok(&format!("{u}/blocks/4"))), vec![4, 3, 2, 1, 0]);
        assert_eq!(heights(&ok(&format!("{u}/blocks/999"))), (3..=12).rev().collect::<Vec<_>>());
        assert_eq!(get(&format!("{u}/blocks/abc")).0, 400);

        // /block/:hash/txs: 25 per page, each as /tx/:txid.
        let txs = ok(&format!("{u}/block/{h5}/txs"));
        assert_eq!(txs.as_array().unwrap().len(), 25);
        assert_eq!(txs[1], ok(&format!("{u}/tx/{}", txs[1]["txid"].as_str().unwrap())));
        assert_eq!(txs[0]["status"]["block_height"], json!(5));
        assert_eq!(txs[0]["status"]["block_hash"], json!(h5));
        let rest = ok(&format!("{u}/block/{h5}/txs/25"));
        assert_eq!(rest.as_array().unwrap().len(), 5);
        assert_eq!(ok(&format!("{u}/block/{h5}/txs/3"))[0], txs[3]);
        assert_eq!(get(&format!("{u}/block/{h5}/txs/30")).0, 400);
        assert_eq!(get(&format!("{u}/block/{h5}/txs/x")).0, 400);
        assert_eq!(get(&format!("{u}/block/{}/txs", "00".repeat(32))).0, 404);
        assert_eq!(get(&format!("{u}/block/zz/txs")).0, 400);

        // Asset names.
        let a = ok(&format!("{u}/asset/{}", named.input[0].issuance_ids().0));
        assert_eq!((&a["name"], &a["ticker"], &a["precision"]), (&json!("Gold"), &json!("GLD"), &json!(2)));
        let a = ok(&format!("{u}/asset/{}", unnamed.input[0].issuance_ids().0));
        assert_eq!((&a["name"], &a["ticker"], &a["precision"]), (&Value::Null, &Value::Null, &Value::Null));
        assert_eq!(a["contract_hash"], json!(hex::encode(contract)));

        // JSON-RPC.
        assert_eq!(rpc(&u, "getblockchaininfo")["result"]["role"], json!("full"));
        rpc(&u, "getnetworkinfo")["result"].clone()
    })
    .await
    .unwrap();
    assert_eq!(
        checks,
        json!({
            "peer_id": "12D3KooSelf", "moniker": "me", "role": "full", "consensus_address": null,
            "listen_addrs": ["/ip4/0.0.0.0/tcp/27000"], "height": 12,
            "peers": [
                { "peer_id": "12D3Koonode-0", "moniker": "node-0", "address": "/ip4/127.0.0.1/tcp/1", "role": "validator",
                  "consensus_address": "ab".repeat(20), "direction": "outbound", "score": 1.5 },
                { "peer_id": "12D3Koonode-2", "moniker": "node-2", "address": "/ip4/127.0.0.1/tcp/1", "role": "full",
                  "consensus_address": null, "direction": "outbound", "score": 1.5 },
            ],
        })
    );

    // No p2p layer: an error, not a hang.
    let err = tokio::task::spawn_blocking(move || rpc(&bare_url, "getnetworkinfo")).await.unwrap();
    assert_eq!(err["error"]["code"], json!(-9), "{err}");

    let _ = stop.send(());
    let _ = bare_stop.send(());
}
