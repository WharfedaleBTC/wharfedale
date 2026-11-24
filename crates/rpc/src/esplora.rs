//! Esplora-compatible REST API (Elements/Liquid flavour), served next to JSON-RPC.
//! Spec: `docs/src/rpc.md`; upstream format: Blockstream esplora `API.md`.
//!
//! `scripthash` = SHA-256(scriptPubKey), hex, not reversed.

use std::str::FromStr;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use elements::confidential::{Asset, Nonce, Value as CValue};
use elements::{encode, Address, AssetId, BlockHash, OutPoint, Script, Transaction, TxOut, Txid};
use serde_json::{json, Value};
use wharfedale_chain::{Block, ADDRESS_PARAMS};
use wharfedale_ledger::{output_kind, OutputKind};
use wharfedale_mempool::Source;
use wharfedale_store::script_hash;

use crate::NodeState;

/// Confirmed history page size and mempool cap (as Esplora).
const CHAIN_PAGE: usize = 25;
const MEMPOOL_CAP: usize = 50;

pub fn router() -> Router<NodeState> {
    Router::new()
        .route("/blocks/tip/height", get(tip_height))
        .route("/blocks/tip/hash", get(tip_hash))
        .route("/block-height/:height", get(block_height))
        .route("/block/:hash", get(block))
        .route("/block/:hash/status", get(block_status))
        .route("/block/:hash/txids", get(block_txids))
        .route("/tx", post(broadcast))
        .route("/tx/:txid", get(tx))
        .route("/tx/:txid/hex", get(tx_hex))
        .route("/tx/:txid/status", get(tx_status))
        .route("/tx/:txid/outspends", get(outspends))
        .route("/tx/:txid/outspend/:vout", get(outspend))
        .route("/scripthash/:hash/txs", get(sh_txs))
        .route("/scripthash/:hash/txs/chain", get(sh_txs_chain))
        .route("/scripthash/:hash/txs/chain/:last", get(sh_txs_chain_after))
        .route("/scripthash/:hash/txs/mempool", get(sh_txs_mempool))
        .route("/scripthash/:hash/utxo", get(sh_utxo))
        .route("/address/:addr/txs", get(addr_txs))
        .route("/address/:addr/utxo", get(addr_utxo))
        .route("/asset/:id", get(asset))
        .route("/mempool/txids", get(mempool_txids))
        .route("/fee-estimates", get(fee_estimates))
}

// ---- plumbing ----

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, self.1).into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, msg.into())
}

fn not_found(msg: impl Into<String>) -> ApiError {
    ApiError(StatusCode::NOT_FOUND, msg.into())
}

fn internal<E: std::fmt::Display>(e: E) -> ApiError {
    ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

/// Runs store/mempool work off the async workers.
async fn blocking<T: Send + 'static>(st: NodeState, f: impl FnOnce(&NodeState) -> ApiResult<T> + Send + 'static) -> ApiResult<T> {
    tokio::task::spawn_blocking(move || f(&st)).await.map_err(internal)?
}

fn parse_txid(s: &str) -> ApiResult<Txid> {
    Txid::from_str(s).map_err(|_| bad("Invalid hex string"))
}

fn parse_sh(s: &str) -> ApiResult<[u8; 32]> {
    hex::decode(s).ok().and_then(|b| b.try_into().ok()).ok_or_else(|| bad("Invalid scripthash"))
}

fn addr_script(s: &str) -> ApiResult<Script> {
    Address::parse_with_params(s, &ADDRESS_PARAMS).map(|a| a.script_pubkey()).map_err(|_| bad("Invalid Wharfedale address"))
}

/// A transaction from the mempool or the chain, with its block height if confirmed.
fn find_tx(st: &NodeState, txid: &Txid) -> ApiResult<Option<(Transaction, Option<u64>)>> {
    if let Some(e) = st.pool().get(txid) {
        return Ok(Some(((*e.tx).clone(), None)));
    }
    Ok(st.store.transaction(txid).map_err(internal)?.map(|(tx, h)| (tx, Some(h))))
}

fn prevout(st: &NodeState, op: &OutPoint) -> ApiResult<Option<TxOut>> {
    Ok(find_tx(st, &op.txid)?.and_then(|(tx, _)| tx.output.into_iter().nth(op.vout as usize)))
}

// ---- JSON shapes ----

fn status(st: &NodeState, height: Option<u64>) -> ApiResult<Value> {
    Ok(match height {
        None => json!({ "confirmed": false }),
        Some(h) => {
            let header = st.store.header(h).map_err(internal)?.ok_or_else(|| internal("missing header"))?;
            json!({ "confirmed": true, "block_height": h, "block_hash": header.hash().to_string(), "block_time": header.timestamp })
        }
    })
}

fn vout_json(out: &TxOut) -> Value {
    let (kind, addr) = match output_kind(out) {
        Some(OutputKind::Taproot) => (
            "v1_p2tr",
            Address::from_script(&out.script_pubkey, None, &ADDRESS_PARAMS).map(|a| a.to_string()),
        ),
        Some(OutputKind::Fee) => ("fee", None),
        Some(OutputKind::OpReturn) => ("op_return", None),
        None => ("unknown", None),
    };
    let mut v = json!({ "scriptpubkey": hex::encode(out.script_pubkey.as_bytes()), "scriptpubkey_type": kind });
    if let Some(a) = addr {
        v["scriptpubkey_address"] = json!(a);
    }
    match out.value {
        CValue::Explicit(n) => v["value"] = json!(n),
        CValue::Confidential(c) => v["valuecommitment"] = json!(c.to_string()),
        CValue::Null => {}
    }
    match out.asset {
        Asset::Explicit(a) => v["asset"] = json!(a.to_string()),
        Asset::Confidential(g) => v["assetcommitment"] = json!(g.to_string()),
        Asset::Null => {}
    }
    match out.nonce {
        Nonce::Explicit(n) => v["nonce"] = json!(hex::encode(n)),
        Nonce::Confidential(pk) => v["noncecommitment"] = json!(pk.to_string()),
        Nonce::Null => {}
    }
    v
}

fn tx_json(st: &NodeState, tx: &Transaction, height: Option<u64>) -> ApiResult<Value> {
    let mut vin = Vec::new();
    for input in &tx.input {
        let coinbase = input.previous_output.is_null();
        let mut v = json!({
            "txid": input.previous_output.txid.to_string(),
            "vout": input.previous_output.vout,
            "is_coinbase": coinbase,
            "scriptsig": hex::encode(input.script_sig.as_bytes()),
            "sequence": input.sequence.0,
            "witness": input.witness.script_witness.iter().map(hex::encode).collect::<Vec<_>>(),
            "is_pegin": input.is_pegin,
            "prevout": if coinbase { Value::Null } else { prevout(st, &input.previous_output)?.map(|o| vout_json(&o)).unwrap_or(Value::Null) },
            "issuance": Value::Null,
        });
        if !input.asset_issuance.is_null() {
            let iss = &input.asset_issuance;
            let (asset, _) = input.issuance_ids();
            let mut i = json!({
                "asset_id": asset.to_string(),
                "is_reissuance": !iss.asset_blinding_nonce.is_null(),
                "asset_blinding_nonce": hex::encode(iss.asset_blinding_nonce.to_byte_array()),
                "asset_entropy": hex::encode(iss.asset_entropy.to_byte_array()),
            });
            if iss.asset_blinding_nonce.is_null() {
                i["contract_hash"] = json!(hex::encode(iss.asset_entropy.to_byte_array()));
            }
            match iss.amount {
                CValue::Explicit(n) => i["assetamount"] = json!(n),
                CValue::Confidential(c) => i["assetamountcommitment"] = json!(c.to_string()),
                CValue::Null => {}
            }
            match iss.inflation_keys {
                CValue::Explicit(n) => i["tokenamount"] = json!(n),
                CValue::Confidential(c) => i["tokenamountcommitment"] = json!(c.to_string()),
                CValue::Null => {}
            }
            v["issuance"] = i;
        }
        vin.push(v);
    }
    let fee: u64 = tx
        .output
        .iter()
        .filter(|o| o.is_fee() && o.asset.explicit() == Some(st.fee_asset))
        .filter_map(|o| o.value.explicit())
        .sum();
    Ok(json!({
        "txid": tx.txid().to_string(),
        "version": tx.version,
        "locktime": tx.lock_time.to_consensus_u32(),
        "size": encode::serialize(tx).len(),
        "weight": tx.weight(),
        "fee": fee,
        "vin": vin,
        "vout": tx.output.iter().map(vout_json).collect::<Vec<_>>(),
        "status": status(st, height)?,
    }))
}

fn block_json(st: &NodeState, block: &Block) -> ApiResult<Value> {
    let h = &block.header;
    Ok(json!({
        "id": h.hash().to_string(),
        "height": h.height,
        "version": h.version,
        "timestamp": h.timestamp,
        "tx_count": block.txs.len(),
        "size": block.encode().len(),
        "weight": block.txs.iter().map(|t| t.weight()).sum::<usize>(),
        "merkle_root": hex::encode(h.tx_merkle_root),
        "previousblockhash": BlockHash::from_byte_array(h.prev_block_hash).to_string(),
        "mediantime": st.store.mtp(h.height).map_err(internal)?,
        "proposer": hex::encode(h.proposer),
    }))
}

fn block_by_hash(st: &NodeState, hash: &str) -> ApiResult<Block> {
    let hash = BlockHash::from_str(hash).map_err(|_| bad("Invalid hex string"))?;
    let height = st.store.height_of(&hash).map_err(internal)?.ok_or_else(|| not_found("Block not found"))?;
    st.store.block(height).map_err(internal)?.ok_or_else(|| not_found("Block not found"))
}

/// Mempool transactions paying to or spending from `script`.
fn mempool_touching(st: &NodeState, script: &[u8; 32]) -> ApiResult<Vec<Transaction>> {
    let txs: Vec<Transaction> = {
        let pool = st.pool();
        pool.txids().iter().filter_map(|t| pool.get(t).map(|e| (*e.tx).clone())).collect()
    };
    let mut out = Vec::new();
    for tx in txs {
        let pays = tx.output.iter().any(|o| &script_hash(&o.script_pubkey) == script);
        let spends = || -> ApiResult<bool> {
            for i in &tx.input {
                if let Some(p) = prevout(st, &i.previous_output)? {
                    if &script_hash(&p.script_pubkey) == script {
                        return Ok(true);
                    }
                }
            }
            Ok(false)
        };
        if pays || spends()? {
            out.push(tx);
            if out.len() >= MEMPOOL_CAP {
                break;
            }
        }
    }
    Ok(out)
}

fn chain_txs(st: &NodeState, sh: &[u8; 32], after: Option<Txid>) -> ApiResult<Vec<Value>> {
    let before = match after {
        None => None,
        Some(txid) => {
            let (_, h) = st.store.transaction(&txid).map_err(internal)?.ok_or_else(|| not_found("Transaction not found"))?;
            let block = st.store.block(h).map_err(internal)?.ok_or_else(|| internal("missing block"))?;
            let pos = block.txs.iter().position(|t| t.txid() == txid).unwrap_or(0) as u32;
            Some((h, pos))
        }
    };
    let rows = st.store.script_history(sh, before, CHAIN_PAGE).map_err(internal)?;
    rows.into_iter()
        .map(|(h, _, txid)| {
            let (tx, _) = st.store.transaction(&txid).map_err(internal)?.ok_or_else(|| internal("indexed tx missing"))?;
            tx_json(st, &tx, Some(h))
        })
        .collect()
}

fn utxos(st: &NodeState, script: &Script) -> ApiResult<Vec<Value>> {
    let entries = st.store.utxos_by_script(script).map_err(internal)?;
    let pool = st.pool();
    let unspent: Vec<_> = entries.into_iter().filter(|(op, _)| pool.spender(op).is_none()).collect();
    drop(pool);
    unspent
        .into_iter()
        .map(|(op, entry)| {
            let mut v = vout_json(&entry.txout);
            v["txid"] = json!(op.txid.to_string());
            v["vout"] = json!(op.vout);
            v["status"] = status(st, Some(entry.height))?;
            // Proofs are not in the UTXO set: read them from the transaction.
            if let Some((tx, _)) = st.store.transaction(&op.txid).map_err(internal)? {
                if let Some(o) = tx.output.get(op.vout as usize) {
                    if let Some(p) = o.witness.rangeproof.as_ref() {
                        v["range_proof"] = json!(hex::encode(p.serialize()));
                    }
                    if let Some(p) = o.witness.surjection_proof.as_ref() {
                        v["surjection_proof"] = json!(hex::encode(p.serialize()));
                    }
                }
            }
            for k in ["scriptpubkey", "scriptpubkey_type", "scriptpubkey_address"] {
                v.as_object_mut().expect("object").remove(k);
            }
            Ok(v)
        })
        .collect()
}

// ---- handlers ----

async fn tip_height(State(st): State<NodeState>) -> ApiResult<String> {
    blocking(st, |st| Ok(st.store.tip().map_err(internal)?.0.to_string())).await
}

async fn tip_hash(State(st): State<NodeState>) -> ApiResult<String> {
    blocking(st, |st| Ok(st.store.tip().map_err(internal)?.1.to_string())).await
}

async fn block_height(State(st): State<NodeState>, Path(h): Path<u64>) -> ApiResult<String> {
    blocking(st, move |st| Ok(st.store.header(h).map_err(internal)?.ok_or_else(|| not_found("Block not found"))?.hash().to_string())).await
}

async fn block(State(st): State<NodeState>, Path(hash): Path<String>) -> ApiResult<Json<Value>> {
    blocking(st, move |st| Ok(Json(block_json(st, &block_by_hash(st, &hash)?)?))).await
}

async fn block_status(State(st): State<NodeState>, Path(hash): Path<String>) -> ApiResult<Json<Value>> {
    blocking(st, move |st| {
        let b = block_by_hash(st, &hash)?;
        let next = st.store.header(b.height() + 1).map_err(internal)?.map(|h| h.hash().to_string());
        Ok(Json(json!({ "in_best_chain": true, "height": b.height(), "next_best": next })))
    })
    .await
}

async fn block_txids(State(st): State<NodeState>, Path(hash): Path<String>) -> ApiResult<Json<Value>> {
    blocking(st, move |st| Ok(Json(json!(block_by_hash(st, &hash)?.txs.iter().map(|t| t.txid().to_string()).collect::<Vec<_>>())))).await
}

async fn broadcast(State(st): State<NodeState>, body: String) -> ApiResult<String> {
    blocking(st, move |st| {
        let bytes = hex::decode(body.trim()).map_err(|_| bad("Invalid hex string"))?;
        let tx: Transaction = encode::deserialize(&bytes).map_err(|e| bad(format!("TX decode failed: {e}")))?;
        st.admit(tx, Source::Local).map(|t| t.to_string()).map_err(|e| bad(format!("sendrawtransaction RPC error: {e}")))
    })
    .await
}

async fn tx(State(st): State<NodeState>, Path(txid): Path<String>) -> ApiResult<Json<Value>> {
    blocking(st, move |st| {
        let (tx, h) = find_tx(st, &parse_txid(&txid)?)?.ok_or_else(|| not_found("Transaction not found"))?;
        Ok(Json(tx_json(st, &tx, h)?))
    })
    .await
}

async fn tx_hex(State(st): State<NodeState>, Path(txid): Path<String>) -> ApiResult<String> {
    blocking(st, move |st| {
        let (tx, _) = find_tx(st, &parse_txid(&txid)?)?.ok_or_else(|| not_found("Transaction not found"))?;
        Ok(hex::encode(encode::serialize(&tx)))
    })
    .await
}

async fn tx_status(State(st): State<NodeState>, Path(txid): Path<String>) -> ApiResult<Json<Value>> {
    blocking(st, move |st| {
        let (_, h) = find_tx(st, &parse_txid(&txid)?)?.ok_or_else(|| not_found("Transaction not found"))?;
        Ok(Json(status(st, h)?))
    })
    .await
}

fn outspend_json(st: &NodeState, op: &OutPoint) -> ApiResult<Value> {
    if let Some(spender) = st.pool().spender(op) {
        let vin = find_tx(st, &spender)?
            .and_then(|(t, _)| t.input.iter().position(|i| i.previous_output == *op))
            .unwrap_or(0);
        return Ok(json!({ "spent": true, "txid": spender.to_string(), "vin": vin, "status": { "confirmed": false } }));
    }
    match st.store.spent_by(op).map_err(internal)? {
        Some((txid, vin)) => {
            let h = st.store.transaction(&txid).map_err(internal)?.map(|(_, h)| h);
            Ok(json!({ "spent": true, "txid": txid.to_string(), "vin": vin, "status": status(st, h)? }))
        }
        None => Ok(json!({ "spent": false })),
    }
}

async fn outspends(State(st): State<NodeState>, Path(txid): Path<String>) -> ApiResult<Json<Value>> {
    blocking(st, move |st| {
        let txid = parse_txid(&txid)?;
        let (tx, _) = find_tx(st, &txid)?.ok_or_else(|| not_found("Transaction not found"))?;
        let v: ApiResult<Vec<Value>> = (0..tx.output.len()).map(|n| outspend_json(st, &OutPoint::new(txid, n as u32))).collect();
        Ok(Json(json!(v?)))
    })
    .await
}

async fn outspend(State(st): State<NodeState>, Path((txid, vout)): Path<(String, u32)>) -> ApiResult<Json<Value>> {
    blocking(st, move |st| Ok(Json(outspend_json(st, &OutPoint::new(parse_txid(&txid)?, vout))?))).await
}

fn history(st: &NodeState, sh: [u8; 32]) -> ApiResult<Vec<Value>> {
    let mut out: Vec<Value> = mempool_touching(st, &sh)?.iter().map(|t| tx_json(st, t, None)).collect::<ApiResult<_>>()?;
    out.extend(chain_txs(st, &sh, None)?);
    Ok(out)
}

async fn sh_txs(State(st): State<NodeState>, Path(hash): Path<String>) -> ApiResult<Json<Value>> {
    blocking(st, move |st| Ok(Json(json!(history(st, parse_sh(&hash)?)?)))).await
}

async fn sh_txs_chain(State(st): State<NodeState>, Path(hash): Path<String>) -> ApiResult<Json<Value>> {
    blocking(st, move |st| Ok(Json(json!(chain_txs(st, &parse_sh(&hash)?, None)?)))).await
}

async fn sh_txs_chain_after(State(st): State<NodeState>, Path((hash, last)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    blocking(st, move |st| Ok(Json(json!(chain_txs(st, &parse_sh(&hash)?, Some(parse_txid(&last)?))?)))).await
}

async fn sh_txs_mempool(State(st): State<NodeState>, Path(hash): Path<String>) -> ApiResult<Json<Value>> {
    blocking(st, move |st| {
        let txs = mempool_touching(st, &parse_sh(&hash)?)?;
        Ok(Json(json!(txs.iter().map(|t| tx_json(st, t, None)).collect::<ApiResult<Vec<_>>>()?)))
    })
    .await
}

async fn sh_utxo(State(st): State<NodeState>, Path(hash): Path<String>) -> ApiResult<Json<Value>> {
    // The UTXO index is keyed by script hash: find the script from any history entry.
    blocking(st, move |st| {
        let sh = parse_sh(&hash)?;
        let Some((_, _, txid)) = st.store.script_history(&sh, None, 1).map_err(internal)?.into_iter().next() else {
            return Ok(Json(json!([])));
        };
        let (tx, _) = st.store.transaction(&txid).map_err(internal)?.ok_or_else(|| internal("indexed tx missing"))?;
        let mut script = tx.output.iter().map(|o| &o.script_pubkey).find(|s| script_hash(s) == sh).cloned();
        if script.is_none() {
            for i in &tx.input {
                if let Some(p) = prevout(st, &i.previous_output)? {
                    if script_hash(&p.script_pubkey) == sh {
                        script = Some(p.script_pubkey);
                        break;
                    }
                }
            }
        }
        match script {
            Some(s) => Ok(Json(json!(utxos(st, &s)?))),
            None => Ok(Json(json!([]))),
        }
    })
    .await
}

async fn addr_txs(State(st): State<NodeState>, Path(addr): Path<String>) -> ApiResult<Json<Value>> {
    blocking(st, move |st| Ok(Json(json!(history(st, script_hash(&addr_script(&addr)?))?)))).await
}

async fn addr_utxo(State(st): State<NodeState>, Path(addr): Path<String>) -> ApiResult<Json<Value>> {
    blocking(st, move |st| Ok(Json(json!(utxos(st, &addr_script(&addr)?)?)))).await
}

async fn asset(State(st): State<NodeState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    blocking(st, move |st| {
        let id = AssetId::from_str(&id).map_err(|_| bad("Invalid asset id"))?;
        if id == st.fee_asset {
            return Ok(Json(json!({ "asset_id": id.to_string(), "fee_asset": true })));
        }
        let r = st.store.asset(&id).map_err(internal)?.ok_or_else(|| not_found("Asset not found"))?;
        Ok(Json(json!({
            "asset_id": r.asset_id,
            "issuance_txin": { "txid": r.issuance_txid, "vin": r.issuance_vin },
            "issuance_prevout": { "txid": r.issuance_prevout.0, "vout": r.issuance_prevout.1 },
            "status": status(st, Some(r.height))?,
            "contract_hash": r.contract_hash,
            "reissuance_token": r.reissuance_token,
            "entropy": r.entropy,
            "chain_stats": {
                "issuance_count": r.issuance_count,
                "issued_amount": r.issued_amount,
                "has_blinded_issuances": r.has_blinded_issuances,
                "reissuance_tokens": r.reissuance_tokens,
            },
        })))
    })
    .await
}

async fn mempool_txids(State(st): State<NodeState>) -> Json<Value> {
    let pool = st.pool();
    Json(json!(pool.txids().iter().map(Txid::to_string).collect::<Vec<_>>()))
}

async fn fee_estimates(State(st): State<NodeState>) -> Json<Value> {
    let rate = st.pool().policy().min_fee_per_kvb as f64 / 1000.0;
    let targets = (1..=25).chain([144, 504, 1008]);
    Json(Value::Object(targets.map(|t| (t.to_string(), json!(rate))).collect()))
}
