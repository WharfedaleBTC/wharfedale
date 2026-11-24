# Architecture

One process, `wharfedale`. Malachite orders blocks, the ledger validates them, redb stores them.

```text
   Other validators                                    Clients / wallets
          │ votes, block parts, sync, txs                     │ JSON-RPC, REST
┌─────────┼───────────────── wharfedale ───────────────────────┼──────────┐
│         ▼                                                    ▼          │
│  ┌──────────────────┐   txs    ┌──────────────┐  submit  ┌──────────┐   │
│  │ Malachite (fork) │◀───────▶│   mempool     │◀─────────│   rpc    │   │
│  │ BFT, WAL, sync,  │          │ pool, policy │          │          │   │
│  │ libp2p channels  │          └──────┬───────┘          └────┬─────┘   │
│  └────────┬─────────┘                 │ check_tx              │ queries │
│           │ AppMsg / replies          ▼                       ▼         │
│  ┌────────┴─────────┐  verify  ┌──────────────┐  UTXOs   ┌──────────┐   │
│  │ node (app loop)  │────────▶│    ledger     │◀────────▶│  store   │   │
│  │ build, commit    │          │ Elements port│          │  redb    │   │
│  └──────────────────┘          └──────┬───────┘          └──────────┘   │
│                                       │                                 │
│          rust-elements · secp256k1-zkp · Simplicity C library           │
└─────────────────────────────────────────────────────────────────────────┘
```

## Crates

| Crate | Responsibility | Depends on |
|---|---|---|
| `wharfedale-ledger` | Consensus rules; `LedgerView` trait; verification cache | elements, ledger-simplicity-sys |
| `wharfedale-ledger-simplicity-sys` | FFI to `simplicity_elements_execSimplicity` (only `unsafe` crate) | simplicity-sys |
| `wharfedale-chain` | `Block`, `Header`, `Genesis`, merkle roots, encoding | ledger |
| `wharfedale-store` | redb tables, atomic commit, indexer | ledger, chain |
| `wharfedale-mempool` | Admission, ordering, eviction, block template | ledger |
| `wharfedale-consensus-ctx` | Malachite `Context` impl | chain, malachite |
| `wharfedale-rpc` | JSON-RPC subset, Esplora REST | ledger, chain, store, mempool |
| `wharfedale-node` | Binary `wharfedale`: wiring, app loop, config | all |
| `wharfedale-wallet-cli` | Binary `wharfedale-wallet`: CLI over the Rust SDK | wharfedale-sdk (`../wharfedale-sdk-rs`) |

Dependency rule: `ledger` depends on nothing in this workspace except the sys crate, and never
on Malachite.

## Transaction lifecycle

1. Client → `rpc` `sendrawtransaction` → `mempool::admit` → `ledger::check_tx` (cached by wtxid).
2. `mempool` publishes a batch on the Malachite `Mempool` channel; peers run `admit` on it.
3. Proposer: `GetValue` → `mempool` template → `ledger::connect_block` dry run → proposal parts.
4. Validators: `ReceivedProposalPart` → `ledger::connect_block` dry run (cache hits) → Valid/Invalid.
5. `Decided` → `store` commits block + certificate + UTXO diff atomically → `mempool::on_block`.

## Block lifecycle

`GetValue` (build, persist, stream) → `ReceivedProposalPart` (assemble, validate, persist) →
`Decided` (commit) → `Finalized` (store extended certificate, start next height).
See [Application loop](consensus/app-loop.md).
