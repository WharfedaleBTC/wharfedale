# Configuration and genesis

The validator set is fixed in `genesis.json`; each node's peers are fixed in `config.toml`.
A node runs as a **validator** (its key must be in genesis) or a **full node** (`role = "full"`:
no validator key; it follows, validates, serves RPC and relays transactions, but never proposes
or votes).

## Home directory

```text
<home>/config.toml
<home>/genesis.json
<home>/keys/validator.json     {"secret": "<32-byte hex>"}  Ed25519 consensus key (validators only)
<home>/keys/node.json          {"secret": "<32-byte hex>"}  libp2p identity (peer id)
<home>/keys/fee.json           {"secret": "<32-byte hex>"}  key of fee_payout_script (testnet tooling)
<home>/data/store.redb         chain state (wharfedale-store)
<home>/data/wal/consensus.wal  Malachite write-ahead log
```

Key files are written with mode 0600 (owner only).

## `genesis.json` (consensus-critical, identical on every node)

```json
{
  "chain_id": "wharfedale-private-1",
  "genesis_time": 1800000000,
  "validators": [
    { "public_key": "<32-byte Ed25519 hex>", "voting_power": 1 }
  ],
  "fee_asset": "<asset id hex>",
  "initial_txs": ["<hex Elements transaction>"]
}
```

| Field | Meaning |
|---|---|
| `chain_id` | Network name; part of the genesis hash |
| `genesis_time` | Unix seconds; genesis header timestamp |
| `validators` | The complete, permanent validator set. Address = first 20 bytes of Keccak-256(public key). The set keeps genesis order, which sets the proposer rotation; each key may appear once |
| `fee_asset` | Asset counted by mempool fee policy. Tooling sets `fee_asset_for(chain_id)` = Elements issuance id from outpoint `00…00:0` with contract hash SHA-256(chain_id); part of the genesis hash |
| `initial_txs` | Elements transactions applied without validation; their taproot outputs become UTXOs at height 0. Tooling: one `genesis_funding_tx` paying the faucet 20 × 10¹² of the fee asset |

**Genesis hash** = hash of the genesis header, whose `prev_block_hash` commits to `chain_id`, `fee_asset`
and the validator set. It is stored in the database, passed to every taproot sighash and to
Simplicity (`genesis_block_hash`), so signatures can't be replayed on another chain.

## `config.toml` (per node)

```toml
moniker           = "node-0"
role              = "validator"                # or "full"; default "validator"
block_time_ms     = 1000                       # Malachite target_time per height
fee_payout_script = "5120…"                    # validators: taproot scriptPubKey for coinbase payouts
create_empty_blocks = false                    # default: blocks only when there are transactions
min_fee_per_kvb   = 10                         # lowest fee rate the mempool accepts, units per 1 000 vbytes

[p2p]
listen_addr           = "/ip4/0.0.0.0/tcp/27000"
persistent_peers      = ["/ip4/10.0.0.2/tcp/27000/p2p/12D3KooW…"]  # peer id required
persistent_peers_only = true

[rpc]                                          # optional; absent = no RPC server
listen_addr = "127.0.0.1:18443"
```

`persistent_peers_only` admits a peer by its **peer id**, in both directions, so every entry
must end in `/p2p/<peer id>` (the id of that node's `keys/node.json`), and a validator must list
the full nodes that connect to it. Discovery is off.

## Full nodes (`role = "full"`)

| | Validator | Full node |
|---|---|---|
| Keys | `validator.json` (in genesis), `node.json` | `node.json` only |
| Network identity | `NetworkIdentity::new_validator` with a validator proof | `NetworkIdentity::new`, no consensus address |
| Consensus | `ConsensusContext::new_validator` (signs votes and proposals) | `ConsensusContext::new_full_node` (verifier only; its address, derived from the node key, is in no validator set, so it is never the proposer) |
| Blocks | Proposes, validates, votes, commits | Validates and commits what the validators decide (gossip and value sync) |
| Mempool, RPC, Esplora, indexer | Yes | Yes: the node to expose publicly, keeping validators' RPC private |
| `fee_payout_script` | Required, taproot | Unused |

Translated into Malachite's configuration (`NodeConfig::engine_config`):

| Malachite setting | Value |
|---|---|
| `consensus.value_payload` | `ProposalAndParts` |
| `consensus.p2p.discovery.enabled` | `false` |
| `consensus.p2p.enable_mempool` | `true` (fork patch) |
| `value_sync.enabled` | `true` (status every 2 s, request timeout 5 s) |
| `HeightParams.target_time` | `block_time_ms` |
| `HeightParams.vote_extension_policy` | `Disabled` |

## Startup checks (`node::start`)

1. Open the store; initialise it from genesis when empty, otherwise require the same
   `schema_version`, `chain_id` and genesis hash.
2. Validator: the address of `keys/validator.json` must be in `genesis.validators`, and
   `fee_payout_script` a taproot script; otherwise the node exits. Full node: its node-key
   address must not collide with a validator.
3. Validator: sign the Malachite validator proof (binds the consensus key to the libp2p peer id).
4. Consensus starts at `tip_height + 1`.

Changing the validator set requires a new genesis.

## CLI

```bash
wharfedale testnet --nodes 4 --home ./testnet --base-port 27000 --rpc-port 18443
```

Local mode: node i listens on `127.0.0.1:base-port+i`, RPC on `rpc-port+i`; `--full-nodes N`
adds N full nodes after the validators (`<home>/full-<i>`).
`--hosts a,b,c,d` (Docker): every node listens on `0.0.0.0:base-port`, peers dial
`/dns4/<host>/…`, RPC on `0.0.0.0:rpc-port`; `--full-hosts x,y` adds full nodes on those hosts.
Every node lists every other as a persistent peer. Also writes `<home>/faucet.json`.

```bash
wharfedale start --home ./testnet/node-0
```
