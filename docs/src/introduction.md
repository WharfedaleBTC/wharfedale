# Wharfedale

Wharfedale is the node for a private layer-1 blockchain.

| Property | Value |
|---|---|
| State model | UTXO |
| Transaction format | Elements (`elements::Transaction`), byte-for-byte |
| Assets | Multi-asset; confidential amounts and asset types |
| Contracts | Simplicity (Elements jets), unchanged; no Bitcoin Script |
| Consensus | Malachite (Tendermint BFT), instant finality |
| Validators | Fixed set in genesis; fixed voting power |
| Rewards | None; the block proposer collects fees |
| Language | Rust |

## Principles

1. **The transaction format is Elements'.** Simplicity, SimplicityHL, rust-elements PSET and
   Liquid wallet tooling work without changes.
2. **Consensus rules are a strict subset of Elements.** Anything not explicitly allowed is
   rejected ([Ledger](ledger/README.md)).
3. **No new cryptography.** Every primitive comes from `secp256k1-zkp`, `rust-elements` or the
   Simplicity C library. This codebase implements rules, state and glue.
4. **The ledger is independent of the consensus engine.** `wharfedale-ledger` has no Malachite
   dependency.

## Reading order

[Architecture](architecture.md) → [Consensus](consensus/README.md) →
[Ledger](ledger/README.md) → [Storage](storage.md) → [Mempool](mempool.md).
