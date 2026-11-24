# Wharfedale

Node for a private layer-1 chain: Malachite BFT consensus over a fixed validator set, an
Elements-format UTXO ledger with confidential assets, and Simplicity contracts. Written in Rust.

- Documentation: `docs/` (mdBook) — `mdbook serve docs`
- Build: `cargo check --workspace` (needs the Malachite fork at `../malachite`)
- Local network in Docker (4 validators, a full node, the web app): `make run`, `make stop`,
  `make clear` (wipes the chain); `make example` walks through it, `make e2e` checks it,
  `make test` checks a fresh chain; `make` lists the targets. RPC on `127.0.0.1:18443`–`18447`,
  web app on <http://localhost:8080>
- Images: CI publishes `ghcr.io/wharfedalebtc/wharfedale` (and the web app's
  `ghcr.io/wharfedalebtc/wharfedale-web`); `../demo-prod` runs a 2-validator network from them

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
