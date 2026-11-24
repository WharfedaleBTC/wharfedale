//! JSON-RPC over HTTP POST `/`. Bitcoin/Elements style: `{"method", "params": [..], "id"}` →
//! `{"result", "error", "id"}` (error = `{"code", "message"}` or null).

use std::net::SocketAddr;

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::methods::{self, RpcError};
use crate::NodeState;

/// Serves JSON-RPC and Esplora until `shutdown` resolves, then closes idle connections, lets
/// in-flight requests finish and returns: every handle on the node state is dropped by then.
pub async fn serve(
    addr: SocketAddr,
    state: NodeState,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let app = Router::new().route("/", post(handle)).merge(crate::esplora::router()).with_state(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "RPC listening");
    axum::serve(listener, app).with_graceful_shutdown(shutdown).await
}

async fn handle(State(state): State<NodeState>, body: axum::body::Bytes) -> Json<Value> {
    let request: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return Json(response(Value::Null, Err(RpcError::new(-32700, format!("parse error: {e}"))))),
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let Some(method) = request.get("method").and_then(Value::as_str).map(str::to_owned) else {
        return Json(response(id, Err(RpcError::new(-32600, "missing method"))));
    };
    let params = match request.get("params") {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(a)) => a.clone(),
        Some(_) => return Json(response(id, Err(RpcError::new(-32602, "params must be an array")))),
    };
    // Validation is CPU-bound: keep it off the async workers.
    let result = tokio::task::spawn_blocking(move || methods::call(&state, &method, &params))
        .await
        .unwrap_or_else(|e| Err(RpcError::new(-32603, format!("internal error: {e}"))));
    Json(response(id, result))
}

fn response(id: Value, result: Result<Value, RpcError>) -> Value {
    match result {
        Ok(result) => json!({ "result": result, "error": null, "id": id }),
        Err(e) => json!({ "result": null, "error": { "code": e.code, "message": e.message }, "id": id }),
    }
}
