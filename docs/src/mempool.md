# Mempool

`wharfedale-mempool`, in memory, shared as `Arc<Mutex<Mempool>>` by RPC, gossip and the app
loop. v1 policy: **confirmed inputs only**, **first seen wins** (no replacement), ordered by fee
rate in the fee asset. There is no dependency graph and no orphan pool.

```rust
pub struct Mempool {
    txs: HashMap<Txid, Entry>,           // Entry { tx, txid, wtxid, vsize, fee, time, source }
    spends: HashMap<OutPoint, Txid>,     // conflict detection
    order: BTreeSet<(fee*1000/vsize, Txid)>, // template (desc) and eviction (asc)
    rejected: HashSet<Wtxid> + FIFO,     // known-invalid, not re-verified
    ..
}
```

## Policy (`Policy::new(fee_asset)`)

| Setting | Default |
|---|---|
| `fee_asset` | `genesis.fee_asset`; only fees in this asset count |
| `min_fee_per_kvb` | 100 fee-asset units per 1000 vbytes |
| `max_pool_vsize` | 300 000 000 |
| `expiry` | 24 h |
| `rejected_cache` | 10 000 wtxids |

## Admission (`Mempool::admit`)

Three steps; cheap checks come before any proof or signature.

| # | Step | Error |
|---|---|---|
| `precheck` (pool lock) | Not already in the pool; wtxid not known-invalid | `Duplicate`, `KnownInvalid` |
| | `ledger::check_tx_stateless` (returns the fee map) | `Invalid(TxError)` (remembered) |
| | Fees only in `fee_asset` | `FeeAsset(asset)` (remembered) |
| | fee ≥ ⌈`min_fee_per_kvb` × vsize / 1000⌉ | `FeeTooLow` |
| | No input spent by a pool transaction; every input confirmed | `Conflict(i, txid)`, `MissingInput(i)` |
| | Room, after evicting strictly lower fee rates | `PoolFull` |
| `verify` (no lock) | `ledger::check_tx` for height tip + 1 (shared `VerifyCache`) | `Invalid(TxError)`; remembered unless lock, maturity, missing input or `SimplicityNodeFault` |
| `insert` (pool lock) | Re-check duplicates, conflicts and inputs (a block or another admission may have intervened); evict; insert | as above |

`Mempool::admit` runs the three in a row. `NodeState::admit` (crate `wharfedale-rpc`) runs them
with the lock released during `verify`, then queues `Source::Local` transactions for gossip.
`NodeState::test_accept` (`testmempoolaccept`) runs `precheck` + `verify` only: the pool is
never modified. Gossiped transactions (`Source::Gossip`) are not re-published. A panic while
the lock was held does not stop the node: `NodeState::pool` takes over a poisoned lock.

## On commit

`remove_for_block(&block.txs)` removes included transactions and any pool transaction spending
an outpoint the block spent; then `expire(now)`.

## Block template (`App::assemble`)

1. `template(MAX_BLOCK_WEIGHT − 4000)`: highest fee rate first, skipping what doesn't fit.
2. `connect_block([placeholder coinbase] + template)`. A failing transaction
   (`BlockError::Tx { index }`) is dropped from the template and the pool; retry.
3. Rebuild the coinbase claiming the fee-asset fees (one output); `connect_block` again (cache hits).

## Gossip

See [Mempool channel](consensus/mempool-channel.md). `node::gossip::receive` admits inbound
batches on a blocking thread; `node::gossip::relay` batches local transactions (50 ms / 64 KiB).

## Later (policy only, no consensus change)

Unconfirmed chains (ancestor packages, orphan pool), replace-by-fee, persistence across restarts.

## Tests

`crates/mempool/tests/mempool.rs`: rejects (duplicate, conflict, unconfirmed input, low fee,
invalid + remembered), fee checked before signatures, fee-asset policy, conflict re-check on
insert, fee-rate ordering, block eviction, expiry, wire round trip.
