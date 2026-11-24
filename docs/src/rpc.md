# RPC

`wharfedale-rpc`: JSON-RPC over HTTP `POST /` (axum), enabled by `[rpc] listen_addr`.

Request/response follow Bitcoin Core: `{"method", "params": [...], "id"}` →
`{"result", "error", "id"}`, where `error` is `null` or `{"code", "message"}`. Parameters are
positional. Methods run on a blocking thread (validation is CPU-bound).

## Methods

| Method | Params | Result |
|---|---|---|
| `getblockchaininfo` | — | `chain`, `blocks`, `bestblockhash`, `genesis_hash`, `fee_asset`, `mempool_size` |
| `getblockcount` | — | tip height |
| `getblockhash` | height | block hash |
| `getblock` | hash, verbosity = 1 | 0: hex of `Block::encode`; 1: header + txids; 2: header + decoded txs |
| `getblockheader` | hash | `hash`, `height`, `previousblockhash`, `time`, `proposer`, `merkleroot`, `witnessmerkleroot`, `confirmations` |
| `sendrawtransaction` | hex | txid (admitted and gossiped) |
| `testmempoolaccept` | [hex, …] | `[{txid, allowed, reject-reason?}]` (not inserted) |
| `getrawtransaction` | txid, verbose = false | hex, or decoded + `hex`, `blockheight`, `confirmations` |
| `decoderawtransaction` | hex | decoded transaction |
| `gettxout` | txid, n, include_mempool = true | output + `confirmations`, `coinbase`, `bestblock`; `null` if spent |
| `getrawmempool` | — | txids, highest fee rate first |
| `getmempoolinfo` | — | `size` |
| `getscriptutxos` | script hex | Wharfedale: unspent outputs paying the script, with `txid`, `vout`, `height`, `coinbase`, `spent_in_mempool` |

Output JSON: `n`, `value_sat` (base units; `null` if confidential), `asset` (`null` if
confidential), `valuecommitment`, `assetcommitment`, `scriptPubKey {hex, type}` with `type` ∈
`witness_v1_taproot`, `fee`, `nulldata`. Differences from Elements: amounts are integers in
`value_sat` (no decimal `value`); blocks use the Wharfedale header.

## Errors (Bitcoin Core codes)

| Code | Meaning |
|---|---|
| -32700 / -32600 / -32601 / -32602 / -32603 | Parse error / bad request / unknown method / bad params / internal |
| -5 | Block or transaction not found |
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
| `GET /block/:hash` | `id`, `height`, `version`, `timestamp`, `tx_count`, `size`, `weight`, `merkle_root`, `previousblockhash`, `mediantime`, `proposer` |
| `GET /block/:hash/status`, `/block/:hash/txids` | `{in_best_chain, height, next_best}`, txids |
| `POST /tx` | body: tx hex → txid (admitted and gossiped) |
| `GET /tx/:txid`, `/tx/:txid/hex`, `/tx/:txid/status` | transaction JSON (mempool or chain), hex, status |
| `GET /tx/:txid/outspends`, `/tx/:txid/outspend/:vout` | `{spent, txid, vin, status}` (mempool spends first) |
| `GET /scripthash/:hash/txs` | ≤ 50 mempool txs + newest 25 confirmed |
| `GET /scripthash/:hash/txs/chain[/:last_seen_txid]`, `/txs/mempool` | paging, mempool only |
| `GET /scripthash/:hash/utxo` | `txid`, `vout`, `status`, `value`/`valuecommitment`, `asset`/`assetcommitment`, `nonce`/`noncecommitment`, `range_proof`, `surjection_proof` |
| `GET /address/:address/txs`, `/address/:address/utxo` | same, for a `wf1…`/`wfc1…` address |
| `GET /asset/:id` | issued asset: `issuance_txin`, `issuance_prevout`, `status`, `contract_hash`, `reissuance_token`, `entropy` (Wharfedale), `chain_stats {issuance_count, issued_amount, has_blinded_issuances, reissuance_tokens}`; the fee asset: `{asset_id, fee_asset: true}` |
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

Not implemented: `/address/:a` and `/scripthash/:h` stats objects, `/tx/:txid/merkle-proof`,
`/block/:hash/txs`, address-prefix search, mempool recent/fee histograms.
