# Wharfedale

Node for a private layer-1 chain: Malachite BFT consensus over a fixed validator set, an
Elements-format UTXO ledger with confidential assets, and Simplicity contracts. Written in Rust.

- Documentation: `docs/` (mdBook) — `mdbook serve docs`
- Build: `cargo check --workspace` (needs the Malachite fork at `../malachite`)
- Local network in Docker: `docker compose -f docker/compose.yml up -d --build`
  (RPC on `127.0.0.1:18443`–`18446`); walkthrough:
  `docker compose -f docker/compose.yml --profile example run --rm example`
- End-to-end test in Docker: `docker/test.sh`

| Crate | Role |
|---|---|
| `crates/ledger` | Consensus rules (Elements subset) |
| `crates/ledger-simplicity-sys` | Simplicity C exec FFI |
| `crates/chain` | Blocks, header, genesis |
| `crates/store` | redb storage, indexer |
| `crates/mempool` | Transaction pool |
| `crates/consensus-ctx` | Malachite `Context` |
| `crates/rpc` | JSON-RPC subset, Esplora REST |
| `crates/node` | `wharfedale` binary |
| `crates/wallet-cli` | `wharfedale-wallet` binary |
