# Development

## Build

```bash
cargo check --workspace
cargo test --workspace
```

Requires the Malachite fork checked out at `../malachite` ([Repositories](repositories.md))
Protobuf is compiled with `protox`, so `protoc` is not needed.

Local 4-validator network:

```bash
cargo run -p wharfedale-node -- testnet --nodes 4 --home ./testnet
```

```bash
cargo run -p wharfedale-node -- start --home ./testnet/node-0   # one terminal per node
```

Docs (mdBook):

```bash
mdbook serve docs
```

## Code rules

| Rule | Why |
|---|---|
| `#![forbid(unsafe_code)]` in every crate except `ledger-simplicity-sys` | FFI is isolated in one crate |
| No new cryptography; primitives only from `elements`, `elements::secp256k1_zkp`, the Simplicity C library | Audited, Liquid-production code |
| `secp256k1-zkp` only via `elements::secp256k1_zkp` | One copy of the library |
| `wharfedale-ledger` never depends on Malachite | Engine-independent core |
| Ported rules cite the Elements C++ function and lines; deviations marked `// WHARFEDALE:` | Reviewable against the reference |
| Consensus code is deterministic: no floats, no wall clock, no `HashMap` iteration order in outputs | Identical results on every validator |
| Every consensus rule has a negative test | The ledger is the inflation boundary |

## Tests

| Kind | Where |
|---|---|
| Unit and negative tests per rule | `crates/ledger/tests/tx.rs`, `crates/ledger/tests/block.rs` (fixtures in `tests/common/`) |
| Confidential amounts, issuance, reissuance, proof tampering | `crates/ledger/tests/confidential.rs` |
| Simplicity FFI: SimplicityHL p2pk executed by the C library; corrupted signature, wrong CMR, budget, garbage program | `crates/ledger-simplicity-sys/tests/exec.rs` |
| Simplicity spends through `check_tx`: p2pk, annex, mixed key-path inputs, lock-time covenant, commitment and shape errors | `crates/ledger/tests/simplicity.rs` |
| Multi-node: 4 validators, stop/restart, catch-up, chain agreement | `crates/node/tests/network.rs` |
| Header, block, genesis, merkle, MTP | `crates/chain/tests/chain.rs` |
| Mempool policy, ordering, eviction, wire format | `crates/mempool/tests/mempool.rs` |
| End to end in process: confidential faucet → alice → bob, issuance, reissuance, Esplora, fee payouts, p2pk contract | `crates/node/tests/transfer.rs` |
| End to end in Docker: 4 containers, same flow plus a timelock contract, chain agreement |
| SDKs: cross-SDK vectors, artifact equivalence, e2e against the Docker network | `../wharfedale-sdk-rs`, `../wharfedale-sdk-ts` ([SDKs](sdk.md)) | `docker/test.sh` ([Docker](docker.md)) |
| Genesis init, atomic commit, reopen, pruning | `crates/store/tests/store.rs` |

## Milestones

| Milestone | Scope |
|---|---|
| M1 ✅ | Ledger: explicit amounts, key-path spends, locks, coinbase and fees, UTXO diff |
| M2 ✅ | Node on Malachite: 4 local validators decide M1 blocks; sync; restart |
| M3 ✅ | Malachite patch + mempool; RPC; wallet CLI with explicit amounts; Docker network and e2e |
| M4 ✅ | Confidential values and assets; issuance and reissuance; wallet blinding; Esplora API |
| M5 ✅ | Simplicity spends (C library via FFI); wallet contracts (SimplicityHL) |
| M6 | Parallel verification, mempool Simplicity limits, property tests and fuzzing, audit |
