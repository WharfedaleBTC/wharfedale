//! Transaction gossip over the Malachite `Mempool` channel (fork patch).
//! Spec: `docs/src/consensus/mempool-channel.md`, `docs/src/mempool.md`.

use std::time::Duration;

use bytes::Bytes;
use elements::{encode, Transaction};
use malachitebft_app_channel::{MempoolInbound, NetworkMsg};
use tokio::sync::mpsc;
use tracing::{debug, warn};
use wharfedale_consensus_ctx::WharfedaleContext;
use wharfedale_mempool::{wire, AdmitError, Source};
use wharfedale_rpc::NodeState;

/// Flush interval for outgoing batches.
pub const BATCH_INTERVAL: Duration = Duration::from_millis(50);

/// Admits every transaction of every inbound batch. Gossiped transactions are not re-published:
/// gossipsub already relays them.
pub async fn receive(mut inbound: mpsc::Receiver<MempoolInbound>, state: NodeState) {
    while let Some(msg) = inbound.recv().await {
        let Some(txs) = wire::decode_batch(&msg.data) else {
            warn!(from = %msg.from, "malformed transaction batch");
            continue;
        };
        let st = state.clone();
        let from = msg.from;
        let _ = tokio::task::spawn_blocking(move || {
            for tx in txs {
                match st.admit(tx, Source::Gossip) {
                    Ok(_) | Err(AdmitError::Duplicate) => {}
                    Err(e) => debug!(%from, "gossiped transaction rejected: {e}"),
                }
            }
        })
        .await;
    }
}

/// Batches locally admitted transactions and publishes them every `BATCH_INTERVAL` or when a
/// batch reaches `wire::MAX_BATCH_BYTES`.
pub async fn relay(mut local: mpsc::Receiver<Transaction>, network: mpsc::Sender<NetworkMsg<WharfedaleContext>>) {
    let mut batch: Vec<Transaction> = Vec::new();
    let mut size = 0;
    let mut tick = tokio::time::interval(BATCH_INTERVAL);
    loop {
        tokio::select! {
            tx = local.recv() => match tx {
                Some(tx) => {
                    size += encode::serialize(&tx).len();
                    batch.push(tx);
                    if size >= wire::MAX_BATCH_BYTES {
                        flush(&mut batch, &mut size, &network).await;
                    }
                }
                None => break,
            },
            _ = tick.tick() => flush(&mut batch, &mut size, &network).await,
        }
    }
}

async fn flush(batch: &mut Vec<Transaction>, size: &mut usize, network: &mpsc::Sender<NetworkMsg<WharfedaleContext>>) {
    if batch.is_empty() {
        return;
    }
    let payload = Bytes::from(wire::encode_batch(batch));
    if network.send(NetworkMsg::PublishTransactions(payload)).await.is_err() {
        warn!("network channel closed; dropping transaction batch");
    }
    batch.clear();
    *size = 0;
}
