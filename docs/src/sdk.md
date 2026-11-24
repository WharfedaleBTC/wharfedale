# SDKs

Two client SDKs, each in its own repository next to this one. Both talk to a node over its
[JSON-RPC and Esplora REST](rpc.md) API.

| Repository | Package | Builds on |
|---|---|---|
| `../wharfedale-sdk-rs` | `wharfedale-sdk` (Rust) | rust-elements 0.27, simplicity-lang 0.9, simplicityhl 0.9 (crates.io only) |
| `../wharfedale-sdk-ts` | `@wharfedale/sdk` (TypeScript, ESM, Node ≥ 20) | liquidjs-lib, secp256k1-zkp (WASM), tiny-secp256k1, @noble/hashes |

## Capabilities

| | Rust | TypeScript |
|---|---|---|
| Keys, `wfc1`/`wf1` addresses, blinding key | ✓ | ✓ |
| RPC + Esplora client | ✓ (blocking) | ✓ (`fetch`) |
| Coins (unblinding), balances | ✓ | ✓ |
| Confidential payments, issuance, reissuance | ✓ (`Transaction::blind`) | ✓ (same algorithm over secp256k1-zkp) |
| Compile SimplicityHL, spend any contract | ✓ | — |
| Spend signature-only contracts from an artifact | ✓ (`signature_artifact`) | ✓ (`SignatureContract`) |
| `sig_all_hash` | rust-simplicity | Port of the C library (`elements/env.c`) |

## Protocol constants

The SDKs don't depend on node crates, so they repeat: address parameters (`wf`/`wfc`), fee
asset derivation, key-path script, blinding-key derivation, contract taproot (NUMS key, leaf
`0xbe`). `crates/node/tests/transfer.rs::sdk_constants_match_the_node` checks the Rust SDK
against the node; `tests/vectors.json` (Rust SDK, generated) is asserted by the TS SDK.

## Interop tests

| Test | Proves |
|---|---|
| `wharfedale-sdk-rs/tests/vectors.rs` | Vectors are current; `sig_all_hash` over explicit, confidential, issuance, reissuance and annex transactions |
| `wharfedale-sdk-ts` `npm test` | TS derives the same keys, addresses, fee assets, contract outputs and `sig_all_hash` |
| `wharfedale-sdk-rs/tests/sdk.rs` | A signature artifact's program equals the satisfied (pruned) program |
| `crates/node/tests/transfer.rs` | The node's own e2e runs through the Rust SDK |
| SDK e2e (`--ignored` / `npm run test:e2e`) on the Docker network | TS-blinded transactions, TS contract spends and node-side Simplicity rejections |

`wharfedale-wallet` (this workspace) is a CLI over the Rust SDK; see [Wallet](wallet.md).
