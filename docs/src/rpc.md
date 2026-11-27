# RPC

`wharfedale-rpc`: JSON-RPC over HTTP `POST /` (axum), enabled by `[rpc] listen_addr`.

Request/response follow Bitcoin Core: `{"method", "params": [...], "id"}` →
`{"result", "error", "id"}`, where `error` is `null` or `{"code", "message"}`. Parameters are
positional. Methods run on a blocking thread (validation is CPU-bound), except
`getnetworkinfo`, which awaits the p2p layer.

## Methods

| Method | Params | Result |
|---|---|---|
| `getblockchaininfo` | — | `chain`, `blocks`, `bestblockhash`, `genesis_hash`, `fee_asset`, `mempool_size`, `role` (`validator` \| `full`); for Bitcoin Core tooling also `headers` (= `blocks`), `pruned: false`, `initialblockdownload: false` (no IBD mode: compare `blocks` with a peer) |
| `getindexinfo` | — | `{txindex: {synced: true, best_block_height}}` (the transaction index is written with each block) |
| `getblockcount` | — | tip height |
| `getblockhash` | height | block hash |
| `getblock` | hash, verbosity = 1 | 0: hex of `Block::encode`; 1: header + txids; 2: header + decoded txs |
| `getblockheader` | hash | `hash`, `height`, `previousblockhash`, `time`, `proposer`, `merkleroot`, `witnessmerkleroot`, `confirmations` |
| `sendrawtransaction` | hex | txid (admitted and gossiped) |
| `testmempoolaccept` | [hex, …] | `[{txid, allowed, reject-reason?}]` (not inserted) |
| `getrawtransaction` | txid, verbose = false | hex, or decoded + `hex`, `blockhash`, `blockheight`, `blocktime`, `confirmations` |
| `decoderawtransaction` | hex | decoded transaction |
| `gettxout` | txid, n, include_mempool = true | output + `confirmations`, `coinbase`, `bestblock`; `null` if spent |
| `getrawmempool` | — | txids, highest fee rate first |
| `getmempoolinfo` | — | `size` |
| `getscriptutxos` | script hex | Wharfedale: unspent outputs paying the script, with `txid`, `vout`, `height`, `coinbase`, `spent_in_mempool` |
| `getnetworkinfo` | — | Wharfedale shape: this node and its connected peers (below) |

Output JSON: `n`, `value_sat` (base units; `null` if confidential), `asset` (`null` if
confidential), `valuecommitment`, `assetcommitment`, `scriptPubKey {hex, type}` with `type` ∈
`witness_v1_taproot`, `fee`, `nulldata`. Differences from Elements: amounts are integers in
`value_sat` (no decimal `value`); blocks use the Wharfedale header.

### `getnetworkinfo`

A Malachite network state dump (`NetworkRequest::dump_state`), reached through
`NodeState::network_info` (set by `node.rs`; the rpc crate does not depend on Malachite). Times
out after 5 s.

```json
{
  "peer_id": "12D3KooW…",              // libp2p peer id
  "moniker": "full-0",
  "role": "validator" | "full",        // config.toml role
  "consensus_address": "20ba…" | null, // lowercase hex; null on full nodes
  "listen_addrs": ["/ip4/127.0.0.1/tcp/27000"],
  "height": 72,                        // committed tip
  "peers": [{                          // sorted by moniker
    "peer_id": "12D3KooW…",
    "moniker": "node-0",
    "address": "/ip4/…/tcp/…/p2p/12D3KooW…",
    "role": "validator" | "full",      // validator: verified proof for a key in the validator set
    "consensus_address": "20ba…" | null,
    "direction": "inbound" | "outbound" | null,
    "score": 200.0                     // GossipSub score
  }]
}
```

A peer is `validator` once its validator proof (sent once, on connection) has been verified
against the current validator set; until then it is listed as `full`.

## Errors (Bitcoin Core codes)

| Code | Meaning |
|---|---|
| -32700 / -32600 / -32601 / -32602 / -32603 | Parse error / bad request / unknown method / bad params / internal |
| -5 | Block or transaction not found |
| -9 | `getnetworkinfo` without a p2p layer |
| -22 | Transaction decode failed |
| -25 | Missing or unconfirmed input |
| -26 | Rejected (invalid, conflict, fee too low, pool full) |
| -27 | Already in the mempool |

## Indexes

`getrawtransaction` (confirmed) uses `tx_index`; `getscriptutxos` uses `spk_utxos`; both are
written in the commit transaction. The Esplora API also uses the indexer's tables
([Storage](storage.md)), so its views lag the tip by at most one indexing pass.

## Esplora REST

Served on the same address as JSON-RPC (`GET` routes; `POST /` stays JSON-RPC). Formats follow
Blockstream Esplora (Elements flavour, `API.md`). Plain-text endpoints return text; the rest JSON;
errors are HTTP 400/404 with a text body.

| Endpoint | Returns |
|---|---|
| `GET /blocks/tip/height`, `/blocks/tip/hash` | text |
| `GET /block-height/:height` | block hash (text) |
| `GET /blocks[/:start_height]` | ≤ 10 block objects (as `/block/:hash`), descending from `start_height` (default and cap: the tip) |
| `GET /block/:hash` | `id`, `height`, `version`, `timestamp`, `tx_count`, `size`, `weight`, `merkle_root`, `previousblockhash`, `mediantime`, `proposer` |
| `GET /block/:hash/status`, `/block/:hash/txids` | `{in_best_chain, height, next_best}`, txids |
| `GET /block/:hash/txs[/:start_index]` | ≤ 25 transactions (as `/tx/:txid`) from `start_index` (default 0); 400 if `start_index` ≥ `tx_count` |
| `POST /tx` | body: tx hex → txid (admitted and gossiped) |
| `GET /tx/:txid`, `/tx/:txid/hex`, `/tx/:txid/status` | transaction JSON (mempool or chain), hex, status |
| `GET /tx/:txid/outspends`, `/tx/:txid/outspend/:vout` | `{spent, txid, vin, status}` (mempool spends first) |
| `GET /scripthash/:hash` | `{scripthash, chain_stats {tx_count}, mempool_stats {tx_count}}` (counts only: amounts are often confidential) |
| `GET /scripthash/:hash/txs` | ≤ 50 mempool txs + newest 25 confirmed |
| `GET /scripthash/:hash/txs/chain[/:last_seen_txid]`, `/txs/mempool` | paging, mempool only |
| `GET /scripthash/:hash/utxo` | `txid`, `vout`, `status`, `value`/`valuecommitment`, `asset`/`assetcommitment`, `nonce`/`noncecommitment`, `range_proof`, `surjection_proof` |
| `GET /address/:address/txs`, `/address/:address/utxo` | same, for a `wf1…`/`wfc1…` address |
| `GET /asset/:id` | issued asset: `issuance_txin`, `issuance_prevout`, `status`, `contract_hash`, `reissuance_token`, `entropy`, `name`, `ticker`, `precision` (Wharfedale; `null` unless named by [WFA1](#asset-names-wfa1)), `chain_stats {tx_count, issuance_count, issued_amount, has_blinded_issuances, reissuance_tokens}`, `mempool_stats {tx_count, issuance_count}`; the fee asset: `{asset_id, fee_asset: true}`. `tx_count`: transactions in the asset's history, (re)issuances and burns (an explicit `OP_RETURN` output of the asset) |
| `GET /asset/:id/txs/chain[/:last_seen_txid]` | the asset's history, newest 25 confirmed per page (issued assets only) |
| `GET /mempool/txids` | txids |
| `GET /fee-estimates` | every target → `min_fee_per_kvb / 1000` (units per vbyte) |

Transaction JSON: `txid`, `version`, `locktime`, `size`, `weight`, `fee` (fee-asset fee outputs),
`vin[] {txid, vout, is_coinbase, scriptsig, sequence, witness[], is_pegin, prevout, issuance}`,
`vout[] {scriptpubkey, scriptpubkey_type (v1_p2tr | fee | op_return), scriptpubkey_address,
value | valuecommitment, asset | assetcommitment, nonce | noncecommitment}`, `status {confirmed,
block_height, block_hash, block_time}`. `issuance`: `asset_id`, `is_reissuance`,
`asset_blinding_nonce`, `asset_entropy`, `contract_hash`, `assetamount`/`assetamountcommitment`,
`tokenamount`/`tokenamountcommitment`.

`:hash` (scripthash) = SHA-256(scriptPubKey), hex, **not reversed**. To confirm against an
existing Esplora client (for example LWK) before relying on it.

Not implemented: the `/address/:a` stats object, funded/spent sums in the `/scripthash/:h` one, `/tx/:txid/merkle-proof`,
address-prefix search, mempool recent/fee histograms.

## Asset names (WFA1)

A convention read by the indexer, not a consensus rule: any transaction carrying it is valid
either way. An issuance transaction names the asset of a **new** issuance input (all-zero
`asset_blinding_nonce`) with an output:

```text
scriptPubKey = OP_RETURN <push "WFA1"> <push metadata>   minimal pushes (Builder::push_slice)
value        = explicit (any; normally 0)
metadata     = UTF-8 JSON, ≤ 128 bytes
contract hash of the input (raw asset_issuance.asset_entropy) == SHA-256(metadata)   raw bytes, not reversed
```

Metadata is a JSON object with `name` (string; 1–64 characters after trimming, no control
characters; stored trimmed), `ticker` (`^[A-Z0-9]{1,8}$`) and `precision` (integer 0–8); other
keys are ignored. A missing, mismatched or invalid output leaves the asset unnamed. Names are set
once, when the issuance is indexed; reissuances never change them. Code:
`wharfedale_store::{wfa1_payload, parse_asset_metadata, asset_metadata}`.
