//! RPC methods. Names and shapes follow Elements where they exist; amounts are base units
//! (`value_sat`). Wharfedale additions: `getscriptutxos`; `getnetworkinfo` has its own shape.

use elements::{confidential, encode, BlockHash, OutPoint, Script, Transaction, TxOut, Txid};
use serde_json::{json, Value};
use wharfedale_chain::{Block, Header};
use wharfedale_mempool::{AdmitError, Source};

use crate::NodeState;

pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

/// Bitcoin Core error codes.
const INVALID_PARAMS: i64 = -32602;
const METHOD_NOT_FOUND: i64 = -32601;
const INTERNAL: i64 = -32603;
const NOT_FOUND: i64 = -5;
const DESERIALIZATION: i64 = -22;
const VERIFY_ERROR: i64 = -25;
const VERIFY_REJECTED: i64 = -26;
const ALREADY_IN_MEMPOOL: i64 = -27;
const P2P_DISABLED: i64 = -9;

type Result<T> = std::result::Result<T, RpcError>;

fn internal<E: std::fmt::Display>(e: E) -> RpcError {
    RpcError::new(INTERNAL, e.to_string())
}

pub fn call(st: &NodeState, method: &str, params: &[Value]) -> Result<Value> {
    match method {
        "getblockchaininfo" => getblockchaininfo(st),
        "getblockcount" => Ok(json!(st.store.tip().map_err(internal)?.0)),
        "getblockhash" => getblockhash(st, params),
        "getblock" => getblock(st, params),
        "getblockheader" => getblockheader(st, params),
        "sendrawtransaction" => sendrawtransaction(st, params),
        "testmempoolaccept" => testmempoolaccept(st, params),
        "getrawtransaction" => getrawtransaction(st, params),
        "decoderawtransaction" => Ok(tx_json(&decode_tx(str_param(params, 0)?)?)),
        "gettxout" => gettxout(st, params),
        "getrawmempool" => {
            let pool = st.pool();
            Ok(json!(pool.txids().iter().map(Txid::to_string).collect::<Vec<_>>()))
        }
        "getmempoolinfo" => {
            let pool = st.pool();
            Ok(json!({ "size": pool.len() }))
        }
        "getscriptutxos" => getscriptutxos(st, params),
        _ => Err(RpcError::new(METHOD_NOT_FOUND, format!("method not found: {method}"))),
    }
}

// ---- params ----

fn str_param(params: &[Value], i: usize) -> Result<&str> {
    params.get(i).and_then(Value::as_str).ok_or_else(|| RpcError::new(INVALID_PARAMS, format!("param {i} must be a string")))
}

fn u64_param(params: &[Value], i: usize) -> Result<u64> {
    params.get(i).and_then(Value::as_u64).ok_or_else(|| RpcError::new(INVALID_PARAMS, format!("param {i} must be an integer")))
}

fn opt_u64(params: &[Value], i: usize, default: u64) -> u64 {
    params.get(i).and_then(|v| v.as_u64().or_else(|| v.as_bool().map(u64::from))).unwrap_or(default)
}

fn opt_bool(params: &[Value], i: usize, default: bool) -> bool {
    params.get(i).and_then(|v| v.as_bool().or_else(|| v.as_u64().map(|n| n != 0))).unwrap_or(default)
}

fn decode_tx(hex_str: &str) -> Result<Transaction> {
    let bytes = hex::decode(hex_str).map_err(|e| RpcError::new(DESERIALIZATION, format!("TX decode failed: {e}")))?;
    encode::deserialize(&bytes).map_err(|e| RpcError::new(DESERIALIZATION, format!("TX decode failed: {e}")))
}

fn parse_txid(s: &str) -> Result<Txid> {
    s.parse().map_err(|_| RpcError::new(INVALID_PARAMS, "invalid txid"))
}

fn block_by_hash(st: &NodeState, hash: &str) -> Result<Block> {
    let hash: BlockHash = hash.parse().map_err(|_| RpcError::new(INVALID_PARAMS, "invalid block hash"))?;
    let height = st.store.height_of(&hash).map_err(internal)?.ok_or_else(|| RpcError::new(NOT_FOUND, "Block not found"))?;
    st.store.block(height).map_err(internal)?.ok_or_else(|| RpcError::new(NOT_FOUND, "Block not available"))
}

// ---- JSON shapes ----

fn header_json(st: &NodeState, h: &Header) -> Result<Value> {
    let (tip, _) = st.store.tip().map_err(internal)?;
    Ok(json!({
        "hash": h.hash().to_string(),
        "height": h.height,
        "version": h.version,
        "previousblockhash": BlockHash::from_byte_array(h.prev_block_hash).to_string(),
        "time": h.timestamp,
        "proposer": hex::encode(h.proposer),
        "merkleroot": hex::encode(h.tx_merkle_root),
        "witnessmerkleroot": hex::encode(h.witness_merkle_root),
        "confirmations": tip + 1 - h.height,
    }))
}

fn txout_json(n: usize, out: &TxOut) -> Value {
    let (asset, assetcommitment) = match out.asset {
        confidential::Asset::Explicit(a) => (json!(a.to_string()), Value::Null),
        confidential::Asset::Confidential(g) => (Value::Null, json!(g.to_string())),
        confidential::Asset::Null => (Value::Null, Value::Null),
    };
    let (value, valuecommitment) = match out.value {
        confidential::Value::Explicit(v) => (json!(v), Value::Null),
        confidential::Value::Confidential(c) => (Value::Null, json!(c.to_string())),
        confidential::Value::Null => (Value::Null, Value::Null),
    };
    let kind = match wharfedale_ledger::output_kind(out) {
        Some(wharfedale_ledger::OutputKind::Taproot) => "witness_v1_taproot",
        Some(wharfedale_ledger::OutputKind::Fee) => "fee",
        Some(wharfedale_ledger::OutputKind::OpReturn) => "nulldata",
        None => "nonstandard",
    };
    json!({
        "n": n,
        "value_sat": value,
        "asset": asset,
        "valuecommitment": valuecommitment,
        "assetcommitment": assetcommitment,
        "scriptPubKey": { "hex": hex::encode(out.script_pubkey.as_bytes()), "type": kind },
    })
}

fn tx_json(tx: &Transaction) -> Value {
    json!({
        "txid": tx.txid().to_string(),
        "wtxid": tx.wtxid().to_string(),
        "version": tx.version,
        "locktime": tx.lock_time.to_consensus_u32(),
        "weight": tx.weight(),
        "vsize": tx.weight().div_ceil(4),
        "vin": tx.input.iter().map(|i| if i.previous_output.is_null() {
            json!({ "coinbase": hex::encode(i.script_sig.as_bytes()), "sequence": i.sequence.0 })
        } else {
            json!({ "txid": i.previous_output.txid.to_string(), "vout": i.previous_output.vout, "sequence": i.sequence.0 })
        }).collect::<Vec<_>>(),
        "vout": tx.output.iter().enumerate().map(|(n, o)| txout_json(n, o)).collect::<Vec<_>>(),
    })
}

// ---- methods ----

fn getblockchaininfo(st: &NodeState) -> Result<Value> {
    let (height, hash) = st.store.tip().map_err(internal)?;
    let pool = st.pool();
    Ok(json!({
        "chain": st.chain_id,
        "blocks": height,
        "bestblockhash": hash.to_string(),
        "genesis_hash": st.genesis_hash.to_string(),
        "fee_asset": st.fee_asset.to_string(),
        "mempool_size": pool.len(),
        "role": st.role.as_str(),
    }))
}

/// Async (asks the p2p layer); called by the server outside `call`.
pub async fn getnetworkinfo(st: &NodeState) -> Result<Value> {
    let ask = st.network_info.as_ref().ok_or_else(|| RpcError::new(P2P_DISABLED, "network info unavailable"))?;
    let mut info = tokio::time::timeout(std::time::Duration::from_secs(5), ask())
        .await
        .map_err(|_| RpcError::new(INTERNAL, "network state request timed out"))?
        .map_err(|e| RpcError::new(INTERNAL, e))?;
    info.peers.sort_by(|a, b| (&a.moniker, &a.peer_id).cmp(&(&b.moniker, &b.peer_id)));
    let (height, _) = st.store.tip().map_err(internal)?;
    Ok(json!({
        "peer_id": info.peer_id,
        "moniker": info.moniker,
        "role": st.role.as_str(),
        "consensus_address": info.consensus_address,
        "listen_addrs": info.listen_addrs,
        "height": height,
        "peers": info.peers.iter().map(|p| json!({
            "peer_id": p.peer_id,
            "moniker": p.moniker,
            "address": p.address,
            "role": if p.validator { "validator" } else { "full" },
            "consensus_address": p.consensus_address,
            "direction": p.direction,
            "score": p.score,
        })).collect::<Vec<_>>(),
    }))
}

fn getblockhash(st: &NodeState, params: &[Value]) -> Result<Value> {
    let h = u64_param(params, 0)?;
    let header = st.store.header(h).map_err(internal)?.ok_or_else(|| RpcError::new(INVALID_PARAMS, "Block height out of range"))?;
    Ok(json!(header.hash().to_string()))
}

fn getblockheader(st: &NodeState, params: &[Value]) -> Result<Value> {
    header_json(st, &block_by_hash(st, str_param(params, 0)?)?.header)
}

fn getblock(st: &NodeState, params: &[Value]) -> Result<Value> {
    let block = block_by_hash(st, str_param(params, 0)?)?;
    match opt_u64(params, 1, 1) {
        0 => Ok(json!(hex::encode(block.encode()))),
        1 => {
            let mut v = header_json(st, &block.header)?;
            v["tx"] = json!(block.txs.iter().map(|t| t.txid().to_string()).collect::<Vec<_>>());
            Ok(v)
        }
        _ => {
            let mut v = header_json(st, &block.header)?;
            v["tx"] = json!(block.txs.iter().map(tx_json).collect::<Vec<_>>());
            Ok(v)
        }
    }
}

fn admit_error(e: AdmitError) -> RpcError {
    match e {
        AdmitError::Duplicate => RpcError::new(ALREADY_IN_MEMPOOL, "txn-already-in-mempool"),
        AdmitError::MissingInput(_) => RpcError::new(VERIFY_ERROR, e.to_string()),
        _ => RpcError::new(VERIFY_REJECTED, e.to_string()),
    }
}

fn sendrawtransaction(st: &NodeState, params: &[Value]) -> Result<Value> {
    let tx = decode_tx(str_param(params, 0)?)?;
    let txid = st.admit(tx, Source::Local).map_err(admit_error)?;
    Ok(json!(txid.to_string()))
}

fn testmempoolaccept(st: &NodeState, params: &[Value]) -> Result<Value> {
    let raw = params.first().and_then(Value::as_array).ok_or_else(|| RpcError::new(INVALID_PARAMS, "param 0 must be an array"))?;
    let mut results = Vec::new();
    for item in raw {
        let tx = decode_tx(item.as_str().ok_or_else(|| RpcError::new(INVALID_PARAMS, "expected hex strings"))?)?;
        let txid = tx.txid();
        // Dry run: the pool is never modified.
        match st.test_accept(tx) {
            Ok(_) => results.push(json!({ "txid": txid.to_string(), "allowed": true })),
            Err(e) => results.push(json!({ "txid": txid.to_string(), "allowed": false, "reject-reason": e.to_string() })),
        }
    }
    Ok(json!(results))
}

fn getrawtransaction(st: &NodeState, params: &[Value]) -> Result<Value> {
    let txid = parse_txid(str_param(params, 0)?)?;
    let verbose = opt_bool(params, 1, false);
    let in_pool = st.pool().get(&txid).map(|e| (*e.tx).clone());
    let (tx, height) = match in_pool {
        Some(tx) => (tx, None),
        None => {
            let (tx, h) = st.store.transaction(&txid).map_err(internal)?.ok_or_else(|| RpcError::new(NOT_FOUND, "No such mempool or blockchain transaction"))?;
            (tx, Some(h))
        }
    };
    if !verbose {
        return Ok(json!(hex::encode(encode::serialize(&tx))));
    }
    let mut v = tx_json(&tx);
    v["hex"] = json!(hex::encode(encode::serialize(&tx)));
    if let Some(h) = height {
        let (tip, _) = st.store.tip().map_err(internal)?;
        v["blockheight"] = json!(h);
        v["confirmations"] = json!(tip + 1 - h);
    } else {
        v["confirmations"] = json!(0);
    }
    Ok(v)
}

fn gettxout(st: &NodeState, params: &[Value]) -> Result<Value> {
    let vout = u32::try_from(u64_param(params, 1)?).map_err(|_| RpcError::new(INVALID_PARAMS, "vout out of range"))?;
    let op = OutPoint::new(parse_txid(str_param(params, 0)?)?, vout);
    let include_mempool = opt_bool(params, 2, true);
    if include_mempool && st.pool().spender(&op).is_some() {
        return Ok(Value::Null);
    }
    let Some(entry) = st.store.utxo_entry(&op).map_err(internal)? else { return Ok(Value::Null) };
    let (tip, hash) = st.store.tip().map_err(internal)?;
    let mut v = txout_json(op.vout as usize, &entry.txout);
    v["bestblock"] = json!(hash.to_string());
    v["confirmations"] = json!(tip + 1 - entry.height);
    v["coinbase"] = json!(entry.is_coinbase);
    Ok(v)
}

fn getscriptutxos(st: &NodeState, params: &[Value]) -> Result<Value> {
    let spk = Script::from(hex::decode(str_param(params, 0)?).map_err(|_| RpcError::new(INVALID_PARAMS, "script must be hex"))?);
    let utxos = st.store.utxos_by_script(&spk).map_err(internal)?;
    let pool = st.pool();
    Ok(json!(utxos
        .iter()
        .map(|(op, e)| {
            let mut v = txout_json(op.vout as usize, &e.txout);
            v["txid"] = json!(op.txid.to_string());
            v["vout"] = json!(op.vout);
            v["height"] = json!(e.height);
            v["coinbase"] = json!(e.is_coinbase);
            v["spent_in_mempool"] = json!(pool.spender(op).is_some());
            v
        })
        .collect::<Vec<_>>()))
}
