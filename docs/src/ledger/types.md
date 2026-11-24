# Types

## From rust-elements (used as-is)

| Type | Wharfedale constraint |
|---|---|
| `Transaction { version, lock_time, input, output }` | `version == 2` |
| `TxIn { previous_output, is_pegin, script_sig, sequence, asset_issuance, witness }` | `is_pegin == false`; `script_sig` empty except in the coinbase |
| `AssetIssuance { asset_blinding_nonce, asset_entropy, amount, inflation_keys }` | Null, new issuance (nonce 0), or reissuance (nonce ≠ 0) |
| `TxInWitness { amount_rangeproof, inflation_keys_rangeproof, script_witness, pegin_witness }` | `pegin_witness` empty |
| `TxOut { asset, value, nonce, script_pubkey, witness }` | Output kinds in [Ledger](README.md) |
| `TxOutWitness { surjection_proof, rangeproof }` | Present iff the asset/value is confidential |
| `confidential::{Asset, Value, Nonce}` | Explicit (`0x01`) or commitment (`0x0a/0x0b`, `0x08/0x09`, `0x02/0x03`) |

- `txid` = SHA-256d of the transaction without witnesses; `wtxid` = with witnesses.
- Proofs live in witnesses. They are committed by the wtxid, the header's witness root and every sighash.

## Wharfedale types (`wharfedale-ledger`)

```rust
pub struct UtxoEntry {
    pub txout: TxOut,       // witness cleared: asset, value, nonce, script_pubkey
    pub height: u64,
    pub is_coinbase: bool,
}

pub trait LedgerView {
    fn utxo(&self, op: &OutPoint) -> Option<UtxoEntry>;
    fn median_time_past(&self, height: u64) -> u64; // MTP of a committed block (BIP68 time locks)
}

pub struct ChainCtx {
    pub height: u64,             // height of the block being validated (tip + 1 in the mempool)
    pub median_time_past: u64,   // MTP of the parent block (BIP113)
    pub genesis_hash: BlockHash, // Simplicity and sighash
    pub coinbase_maturity: u64,  // 1
}

pub type FeeMap = BTreeMap<AssetId, u64>;

pub struct BlockDiff {
    pub spent: Vec<(OutPoint, UtxoEntry)>,
    pub created: Vec<(OutPoint, UtxoEntry)>,
    pub fees: FeeMap,
}

pub struct VerifyCache; // wtxids whose amounts and spends all verified; bounded (FIFO, 100 000)
```

`MemoryLedger` is an in-memory `LedgerView` for tests, tools and mempool scratch state.

The cache is keyed by wtxid. A wtxid commits to every prevout, and an outpoint's output never
changes (txids are unique; BIP34 makes coinbase txids unique). So a cached result holds in any
state where the inputs exist. An entry is written only after the whole `check_tx` passed, so
invalid transactions never occupy it; one cache serves one chain (the genesis hash is in every
sighash, not in the key).

## Errors

`TxError` has one variant per rule (for example `PeginForbidden`,
`LeafVersionForbidden(u8)`, `BalanceMismatch`, `RangeproofMinValueZero { output }`,
`ReissuanceTokenMismatch { input }`). Each rule's variant is named in [Validation](validation.md).
