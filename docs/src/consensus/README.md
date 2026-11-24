# Consensus

Malachite (Tendermint BFT, Rust) decides one block per height over the fixed genesis validator set.

| Property | Value |
|---|---|
| Engine | `arc-malachitebft-*` from our fork, channel-based integration (`app-channel`) |
| Finality | Instant: a block with commit signatures from > 2/3 voting power is final |
| Fault tolerance | f Byzantine of 3f + 1 voting power; halts if > 1/3 is offline |
| Validator set | `genesis.validators`, returned unchanged in every `HeightParams` |
| Proposer selection | Round-robin over the set sorted by address: index `(height − 1 + round) mod n` |
| Signing | Ed25519 (`arc-malachitebft-signing-ed25519`) |
| Value | `Value(ValueId)` = block hash; the full block travels as signed proposal parts |
| Rewards | None. A coinbase, present only when the block pays fees, claims at most them |
| Empty blocks | None: a height starts only when there is a transaction ([holding a height](app-loop.md#holding-a-height)) |
| Misbehaviour | `Finalized.evidence` (double vote/proposal) is logged and alerted; no slashing |
| Reorgs | None |

## What Malachite provides vs what the app provides

| Malachite | Wharfedale |
|---|---|
| Tendermint state machine, timeouts, vote keeping | Block format, building and validation |
| Write-ahead log for the current height | Committed blocks, certificates, state |
| libp2p network: Consensus, ProposalParts, Liveness, Sync (+ Mempool in the fork) | Mempool logic, transaction encoding |
| Value sync (requests/responses) | Serving and validating synced blocks |

## Pages

- [Application loop](app-loop.md): the `AppMsg` handlers.
- [Blocks and proposals](blocks.md): header, hash, parts, time rules.
- [Malachite patch: Mempool channel](mempool-channel.md).
