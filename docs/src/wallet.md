# Wallet

`wharfedale-wallet-cli`: binary `wharfedale-wallet`, a thin CLI over the Rust SDK
(`wharfedale-sdk`, [SDKs](sdk.md)). Single-key, client-side, over the node's
[JSON-RPC and Esplora REST](rpc.md) (same base URL).

## Keys and addresses

| Item | Definition |
|---|---|
| Key file | `{"secret": "<32-byte hex>"}` (same format as node keys) |
| Output script | Key-path-only taproot: `OP_1 <Q>`, `Q = P + H_TapTweak/elements(P)·G` (`chain::key_path_script`) |
| Blinding key | `SHA-256("wharfedale/blinding-key" ‖ secret)` (`blinding_key`) |
| Confidential address | `wfc1p…`: blech32 of the output script + blinding public key (`address`); default |
| Explicit address | `wf1p…`: bech32m of the same script (`explicit_address`) |

Address parameters: `chain::ADDRESS_PARAMS` (hrp `wf`, blech hrp `wfc`).

## Commands

```bash
wharfedale-wallet --rpc http://127.0.0.1:18443 keygen --out alice.json
```

| Command | Does |
|---|---|
| `keygen --out FILE` | New key; prints the confidential address |
| `address --key FILE [--explicit \| --script]` | `wfc1…` (default), `wf1…`, or output script hex |
| `balance --key FILE [--asset ID]` | Unspent per asset; confidential coins unblinded; excludes coins spent in the mempool |
| `send --key FILE --to ADDR\|SCRIPT --amount N [--asset ID] [--fee N]` | Pay; prints txid. Default asset: `fee_asset`; default fee 100 (`wharfedale_sdk::wallet::DEFAULT_FEE`) |
| `issue --key FILE --amount N [--tokens T] [--fee N]` | New asset (explicit amounts) to self, confidentially; prints txid, asset id, token id |
| `reissue --key FILE --asset ID --amount N [--fee N]` | Reissue with a held token (token returned to self); prints txid |
| `pubkey --key FILE` | x-only public key (untweaked), for contract arguments |
| `contract-address --program F.simf [--args A.json] [--script \| --cmr]` | Contract address `wf1p…`, output script hex, or CMR hex |
| `contract-balance --program F.simf [--args A.json] [--asset ID]` | Coins locked to the contract |
| `contract-artifact --program F.simf [--args A.json]` | `{cmr, program, signatures}` of a signature-only contract, for the TypeScript SDK |
| `contract-spend --program F.simf [--args A.json] [--witness W.json] --to ADDR --amount N [--asset ID] [--fee N] [--lock-time N]` | Spend contract coins; prints txid |
| `wait-tx --txid T [--timeout S]` | Prints the block height once confirmed |
| `wait-height --height H [--timeout S]` | Waits for the chain |
| `rpc METHOD [PARAMS…]` | Any JSON-RPC call; params parsed as JSON when possible |
| `rest PATH` | Any Esplora GET, e.g. `rest asset/<id>` |

## Coins (`coins`)

`getscriptutxos(own script)` → for each output not spent in the mempool, fetch its transaction
(`getrawtransaction`): explicit outputs open with zero blinding factors; confidential ones are
unblinded with the blinding key (rangeproof rewind). Outputs blinded to other keys are skipped.

## Building a transaction (`send`, `issue`, `reissue` → `finish`)

1. `getblockchaininfo` → `fee_asset`, `genesis_hash`.
2. Select coins largest first per needed asset (payment; fee in `fee_asset`).
3. Outputs are created **explicit**; an output to blind carries its blinding public key in
   `nonce` (Elements convention). Payment: blinded if the destination is `wfc1`. Change: blinded
   to self if the transaction is confidential (confidential destination or any confidential
   input). Fee: explicit.
4. If any output is marked, `Transaction::blind` (rust-elements) with the surjection domain in
   consensus order: each input's opening, then its issuance pseudo-inputs (asset, then token).
   A transaction spending confidential coins must have at least one blinded output.
5. Sign every input: Elements taproot key-spend sighash (`SIGHASH_DEFAULT`, genesis hash) →
   BIP340 with the tweaked key.
6. `sendrawtransaction`.

**Issue**: input 0 carries `AssetIssuance { nonce: 0, entropy: 0 (contract hash), amount,
inflation_keys }`; asset and token ids from `TxIn::issuance_ids`; both paid to self, blinded
(the token must be confidential to be reissued).

**Reissue**: `GET /asset/:id` → token id and entropy; input 0 spends a confidential token coin
with `AssetIssuance { nonce: token asset blinding factor, entropy, amount, inflation_keys: null }`;
the token returns to self.

## Contracts (`contract`)

SimplicityHL source + arguments (`{"NAME": "value"}`, values as strings) → `Contract`
(compiled program, CMR). Output script and control block from
`ledger_simplicity_sys::taproot` ([Simplicity](ledger/simplicity.md#contract-outputs)).
Contract outputs are explicit; only explicit coins are found by `contract-balance`/`-spend`.

`contract-spend`:

1. Select contract coins for `amount` (asset) and `fee` (fee asset); change returns to the
   contract; payment blinded if `to` is `wfc1`.
2. `--lock-time N`: nLockTime = N, sequences `0xfffffffe`: a height below 500 000 000 (for
   `check_lock_height`), a unix time at or above (for `check_lock_time`; compared with the
   block's timestamp).
3. Per input: rust-simplicity `ElementsEnv` (tx, all spent outputs, ix, CMR, control block,
   genesis) → `sig_all_hash`. In the witness JSON, every `"sign:<key file>"` string becomes a
   BIP340 signature by that key (untweaked) over it.
4. `satisfy_with_env` (prunes; fails locally if the witness doesn't satisfy the program) →
   `[witness, program, cmr, control_block]`.
5. `sendrawtransaction`.

```bash
wharfedale-wallet pubkey --key bob.json
```

```bash
wharfedale-wallet contract-address --program examples/contracts/p2pk.simf --args p2pk.args
```

```bash
wharfedale-wallet contract-spend --program examples/contracts/p2pk.simf --args p2pk.args --witness bob.wit --to "$ALICE" --amount 200000
```

`p2pk.args`: `{"OWNER": "0x<pubkey>"}`; `bob.wit`: `{"SIG": "sign:bob.json"}`.

v1 mempool accepts confirmed inputs only: a wallet can't spend its change until the next block.
