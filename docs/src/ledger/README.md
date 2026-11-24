# Ledger

`wharfedale-ledger` validates `elements::Transaction`s under a strict subset of Elements'
consensus rules and applies them to the UTXO set. Anything not listed as allowed is rejected.

## Elements subset

| Elements feature | Wharfedale |
|---|---|
| Confidential values and assets; rangeproofs; surjection proofs | Allowed, identical |
| Asset issuance and reissuance (explicit or confidential amounts) | Allowed, identical |
| Explicit fee outputs (empty scriptPubKey) | Allowed, identical |
| nLockTime; BIP68 relative locks (version 2) | Enforced, identical |
| Taproot key-path spend (BIP340, Elements sighash) | Allowed |
| Taproot script path, leaf version `0xbe` (Simplicity) | Allowed |
| Taproot script path, leaf `0xc4` (Tapscript) or any other version | **Rejected** |
| Legacy, P2SH, segwit v0, bare scripts; non-empty `scriptSig` | **Rejected** |
| Peg-ins, peg-in witnesses, PAK | **Rejected** |
| `OP_RETURN` outputs | Allowed with explicit asset and value only |
| Coinbase claiming fees | Allowed: ≤ fee map, subsidy 0 |
| Dynafed, signblockscript | Not applicable (header is Wharfedale's) |

## Output kinds

| Kind | `script_pubkey` | Asset / value | Stored in UTXO set |
|---|---|---|---|
| Taproot | `OP_1 <32-byte Q>` | Explicit or confidential | Yes |
| Fee | empty | Explicit, value > 0 | No; claimed by the coinbase |
| Burn / data | `OP_RETURN …` | Explicit | No |

## Public API

```rust
pub fn check_tx_stateless(tx: &Transaction) -> Result<(), TxError>;
pub fn check_tx(view: &impl LedgerView, tx: &Transaction, ctx: &ChainCtx,
                cache: &VerifyCache) -> Result<FeeMap, TxError>;
pub fn connect_block(view: &impl LedgerView, txs: &[Transaction], ctx: &ChainCtx,
                     cache: &VerifyCache) -> Result<BlockDiff, BlockError>;
```

`connect_block` is pure: it returns a `BlockDiff` (spent and created UTXOs, fee map), and
`wharfedale-store` applies it. The same call serves block building, proposal validation and
sync.

## Implementation status

| Milestone | Status |
|---|---|
| M1: explicit amounts, key-path spends, locks, coinbase and fees, `BlockDiff` | Done |
| M4: confidential values and assets, issuance, reissuance | Done (`amounts.rs`: port of `VerifyAmounts`) |
| M5: Simplicity spends | Done (`spend.rs` → `wharfedale-ledger-simplicity-sys` → C `execSimplicity`) |

## Module map

| Module | Pipeline stage | Elements source |
|---|---|---|
| `stateless` | 1, coinbase shape | `CheckTransaction`, `HasValidFee` |
| `context` | 2–3 | `CheckTxInputs` (maturity), `IsFinalTx`, `CalculateSequenceLocks` |
| `amounts` | 4 | `VerifyAmounts`, `VerifyIssuanceAmount`, `VerifyRangeProof` |
| `spend` | 5 | `VerifyWitnessProgram`, taproot branch |
| `block` | block | `ConnectBlock` loop, `VerifyCoinbaseAmount` |

## Pages

- [Types](types.md)
- [Cryptography](cryptography.md)
- [Validation](validation.md): the check pipeline and its map to Elements' C++.
- [Simplicity](simplicity.md)
