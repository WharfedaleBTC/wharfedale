# Storage

`wharfedale-store`: redb 2.6 (pure Rust, ACID, single writer, MVCC readers). Malachite's WAL
covers only the in-progress height; everything committed lives here.

Status: implemented. `spk_utxos` and `tx_index` are written in the commit transaction; the
Esplora indexes are written by the background indexer.

## Tables

**Core: every node, written only in the commit transaction.**

| Table | Key → Value |
|---|---|
| `meta` | name → bytes: `schema_version`, `chain_id`, `genesis_hash`, `tip_height`, `tip_hash` |
| `headers` | height (u64) → `Header::encode()` (136 B) |
| `bodies` | height → `Block::encode()` (prunable) |
| `block_index` | block hash → height |
| `certs` | height → protobuf `ExtendedCommitCertificate`; rewritten on `Finalized` (never pruned) |
| `utxos` | outpoint (`txid ‖ vout LE`, 36 B) → `height LE ‖ is_coinbase ‖ TxOut consensus bytes` |
| `undo` | height → spent entries (last 1 000 blocks; operator rollback tool only) |

**In-flight: every node; keys start with the height (BE) and every key ≤ the committed height
is deleted in the commit transaction.**

| Table | Key → Value |
|---|---|
| `built_proposals` | height ‖ round → block hash we proposed |
| `undecided` | height ‖ round ‖ hash → proposal record (proposer, round, valid_round, validity, block) |
| `undecided_parts` | height ‖ hash → signed proposal parts (for restreaming) |

Proposals for a future height are buffered in memory and validated when that height starts.

`Store` implements `LedgerView`: `utxo` reads `utxos`; `median_time_past(h)` is the median of
the timestamps of headers `h-10..=h`.

**Query indexes.** Implemented, in the commit transaction:

| Table | Key → Value | Serves |
|---|---|---|
| `spk_utxos` | SHA-256(scriptPubKey) ‖ outpoint → () | `getscriptutxos` |
| `tx_index` | txid → height (u64 LE) ‖ position (u32 LE) | `getrawtransaction` |

Written by the indexer (`Store::index_next`, one block per write transaction):

| Table | Key → Value | Serves |
|---|---|---|
| `tx_index` | txid → (height, position) | `getrawtransaction`, `/tx/:txid` |
| `spk_history` | scripthash ‖ height (BE) ‖ position (BE) ‖ txid → () | `/scripthash/:hash/txs`, `/address/:a/txs` |
| `spent_by` | outpoint → txid ‖ vin (LE) | `/tx/:txid/outspends` |
| `assets` | asset id → JSON `AssetRecord` (below) | `/asset/:id`, wallet reissuance |
| `meta.indexed_height` | last indexed height | indexer cursor |

A script appears in `spk_history` for every transaction that pays it or spends from it.

`AssetRecord` (JSON):

| Field | Meaning |
|---|---|
| `asset_id`, `reissuance_token` | ids (hex) |
| `issuance_txid`, `issuance_vin`, `issuance_prevout` | first issuance input and the outpoint it spent |
| `contract_hash`, `entropy` | hex; `entropy` is needed to reissue |
| `height` | block of the first issuance |
| `issuance_count`, `issued_amount`, `has_blinded_issuances`, `reissuance_tokens` | totals over (re)issuances; amounts explicit only |
| `name`, `ticker`, `precision` | optional, from a [WFA1](rpc.md#asset-names-wfa1) output of the issuance transaction; never changed by reissuances. Missing in older records (decode as `null`) |

`scripthash` = SHA-256(scriptPubKey), the Electrum/Esplora convention.

## Commit (`Store::commit`, on `Decided`)

```text
diff = cache[block_hash] or ledger::connect_block(store, block)      (node)
require block.height == tip_height + 1 and block.prev == tip_hash   (else NotNext)
write txn:
  headers, bodies, block_index, certs  ← put
  utxos                                ← delete diff.spent; insert diff.created
  undo[h] ← diff.spent; delete undo[h − 1000]
  meta.tip_height, meta.tip_hash       ← h, hash
  in-flight tables                     ← delete keys ≤ h
commit (redb default durability: fsync) → reply(()) → [M3: mempool.on_block, indexer.notify(h)]
```

`Store::init_or_check(genesis)` writes genesis the same way (taproot outputs of `initial_txs`
become UTXOs at height 0).

## Indexer

A node task (`node.rs`) runs `Store::index_next` until it returns `None`, then waits for the next
committed tip. Per block: read the block, resolve spent outputs through `tx_index`, update
`spk_history`, `spent_by`, `assets`, advance `indexed_height` — one write transaction. It never
blocks consensus and can be rebuilt by dropping the three tables and the cursor.

## Pruning

`prune_bodies = N` keeps the last N bodies. `GetHistoryMinHeight` reports the lowest stored
body. At least two archive nodes keep full history for sync. Certificates are never pruned.

## Startup

`init_or_check`: initialise from genesis when empty; otherwise require the same
`schema_version`, `chain_id` and genesis hash (`WrongChain`, `Schema`).

## State root (open decision)

`state_root` = rolling multiset hash of the UTXO set after the block (MuHash3072 or a vetted
LtHash; never XOR or additive SHA-256). It is updated per UTXO change, O(1) each. Validators
reject a proposal whose `state_root` differs from their own, so state divergence halts the chain
instead of splitting it.
