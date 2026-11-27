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
use wharfedale_ledger::{BlockDiff};
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

/// Esplora's asset history (`tx_count`, `/asset/:id/txs/chain` paging), the scripthash summary,
/// and the Bitcoin Core fields of `getrawtransaction`, `getblockchaininfo` and `getindexinfo`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn asset_history_scripthash_stats_and_core_fields() {
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

    // Block 1 issues the asset; blocks 2..=28 each burn some of it (27 burns, so the history
    // spans two pages); block 29 is unrelated; block 30 reissues.
    let issue = tx(1, 0, Some([7; 32]), None);
    let (asset, _) = issue.input[0].issuance_ids();
    let burn = |h: u64| {
        let mut o = out(Builder::new().push_opcode(opcodes::all::OP_RETURN).into_script());
        o.asset = Asset::Explicit(asset);
        o.value = CValue::Explicit(1);
        tx(h, 0, None, Some(o))
    };
    commit(&store, vec![issue.clone()]);
    let burns: Vec<Transaction> = (2..=28).map(burn).collect();
    for b in &burns {
        commit(&store, vec![b.clone()]);
    }
    commit(&store, vec![tx(29, 0, None, None)]);
    let mut reissue = tx(30, 0, None, None);
    let entropy = elements::AssetId::generate_asset_entropy(issue.input[0].previous_output, elements::ContractHash::from_byte_array([7; 32]));
    reissue.input[0].asset_issuance = AssetIssuance {
        asset_blinding_nonce: elements::AssetBlindingNonce::from_byte_array([9; 32]),
        asset_entropy: elements::AssetEntropy::from_byte_array(entropy.to_byte_array()),
        amount: CValue::Explicit(500),
        inflation_keys: CValue::Null,
    };
    assert_eq!(reissue.input[0].issuance_ids().0, asset);
    let last = commit(&store, vec![reissue.clone()]);
    while store.index_next().unwrap().is_some() {}
    assert!(!store.check_index_version().unwrap(), "a current index is kept");
    let store = Arc::new(store);

    let (u, stop) = serve(state(store, None));
    wait_up(&u).await;
    let asset_s = asset.to_string();
    tokio::task::spawn_blocking(move || {
        let a = ok(&format!("{u}/asset/{asset_s}"));
        assert_eq!(a["chain_stats"]["tx_count"], json!(29), "{a}");
        assert_eq!(a["chain_stats"]["issuance_count"], json!(2), "{a}");
        assert_eq!(a["mempool_stats"], json!({ "tx_count": 0, "issuance_count": 0 }));

        // The asset registry lists every issued asset, newest first, with the total in a header.
        let listed = ureq::get(&format!("{u}/assets/registry?start_index=0&limit=10")).call().unwrap();
        assert_eq!(listed.header("x-total-results"), Some("1"));
        let listed: Value = listed.into_json().unwrap();
        assert_eq!(listed.as_array().unwrap().len(), 1);
        assert_eq!(listed[0]["asset_id"], json!(asset_s));
        assert_eq!(listed[0]["chain_stats"]["issuance_count"], json!(2));
        assert!(listed[0].get("mempool_stats").is_none());
        assert_eq!(ok(&format!("{u}/assets/registry?start_index=5")), json!([]));

        // Newest first, 25 per page, then the rest after the last seen txid.
        let page = ok(&format!("{u}/asset/{asset_s}/txs/chain"));
        let ids = |v: &Value| v.as_array().unwrap().iter().map(|t| t["txid"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
        let first = ids(&page);
        assert_eq!(first.len(), 25);
        assert_eq!(first[0], reissue.txid().to_string());
        assert_eq!(first[1], burns[26].txid().to_string());
        assert_eq!(page[0]["status"]["block_hash"], json!(last.hash().to_string()));
        let rest = ids(&ok(&format!("{u}/asset/{asset_s}/txs/chain/{}", first[24])));
        assert_eq!(rest.len(), 4);
        assert_eq!(rest.last().unwrap(), &issue.txid().to_string());
        assert!(ids(&ok(&format!("{u}/asset/{asset_s}/txs/chain/{}", rest[3]))).is_empty());
        assert_eq!(get(&format!("{u}/asset/{}/txs/chain", "00".repeat(32))).0, 404);

        // Every test transaction pays the same Taproot script: 30 of them.
        let sh = hex::encode(sha256::Hash::hash(taproot().as_bytes()).to_byte_array());
        let stats = ok(&format!("{u}/scripthash/{sh}"));
        assert_eq!(stats, json!({ "scripthash": sh, "chain_stats": { "tx_count": 30 }, "mempool_stats": { "tx_count": 0 } }));
        assert_eq!(ok(&format!("{u}/scripthash/{}", "11".repeat(32)))["chain_stats"]["tx_count"], json!(0));

        // Bitcoin Core fields.
        let call = |method: &str, params: Value| -> Value {
            ureq::post(&u).send_json(json!({ "method": method, "params": params, "id": 1 })).unwrap().into_json::<Value>().unwrap()["result"].clone()
        };
        let raw = call("getrawtransaction", json!([reissue.txid().to_string(), true]));
        assert_eq!(raw["blockhash"], json!(last.hash().to_string()));
        assert_eq!(raw["blockheight"], json!(30));
        assert_eq!(raw["blocktime"], json!(last.header.timestamp));
        let info = call("getblockchaininfo", json!([]));
        assert_eq!((&info["headers"], &info["pruned"], &info["initialblockdownload"]), (&json!(30), &json!(false), &json!(false)));
        assert_eq!(call("getindexinfo", json!([])), json!({ "txindex": { "synced": true, "best_block_height": 30 } }));
    })
    .await
    .unwrap();
    let _ = stop.send(());
}
