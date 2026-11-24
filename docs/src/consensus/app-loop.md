# Application loop

`wharfedale-node` (`app.rs`) runs one task over `channels.consensus` (`AppMsg<Ctx>`).

Wiring (`node.rs`), as Malachite's test app:
- `EngineBuilder::new(WharfedaleContext, engine_config)`
- `.with_default_wal(WalContext::new(<home>/data/wal/consensus.wal, ProtobufCodec))`
- `.with_default_network(NetworkContext::new(NetworkIdentity::new_validator(..), ProtobufCodec))`
- `.with_default_consensus(ConsensusContext::new_validator(address, Ed25519Verifier, Ed25519Signer))`
- `.with_default_sync(SyncContext::new(ProtobufCodec))`
- `.with_default_request(RequestContext::new(100))`
- `.build()` → `(Channels, EngineHandle)`

## Handlers

| `AppMsg` | Handler |
|---|---|
| `ConsensusReady` | Reply `(tip_height + 1, height_params())` |
| `StartedRound` | Record (height, round). Validate buffered parts for this height. Reply with the stored undecided proposals for (height, round) |
| `GetValue` | `App::propose`: if `built_proposals[(h, r)]` exists, return that block. Else build (coinbase; mempool template from M3), dry-run `ledger::connect_block`, **persist** proposal + parts + built marker, reply `LocallyProposedValue(block hash)`, publish parts |
| `ReceivedProposalPart` | Reassemble. Past height: drop. Height + 1: buffer if proposer, signature and structure check (≤ 16 buffered). Further ahead: drop. Current height: `App::accept_parts` — proposer, signature and structure check (else ignore); a block hash already stored for this height reuses its verdict; otherwise header + `connect_block` → `Valid`/`Invalid`; **persist**; reply `ProposedValue` |
| `RestreamProposal` | Republish the stored parts |
| `Decided` | `App::commit`: atomic commit (block, extended certificate, UTXO diff, tip); then reply |
| `Finalized` | `App::commit` (idempotent: for a committed height, checks the certificate is for the stored block and updates it; a failed update is logged); log evidence; reply `Next::Start(h + 1)`, or `Next::Restart(h)` if the commit failed |
| `GetHistoryMinHeight` | Lowest stored body height (≥ 1) |
| `GetDecidedValues` | `RawDecidedValue { value_bytes: Block::encode(), certificate }` per stored height |
| `ProcessSyncedValue` | Decode, height or `roots_match` fails → `PeerFault`; not the next height → `LocalTransientError`; else full validation without the wall-clock check, persist, `Verdict(ProposedValue)` |
| `ExtendVote` | `None` (vote extensions disabled) |
| `VerifyVoteExtensions` | Accept |

A handler error (storage, network) is logged and the loop continues; the dropped reply makes
consensus time out or retry. Only the consensus channel closing ends `run`.

`height_params()` = `HeightParams::new(genesis validator set, LinearTimeouts::default(),
Some(block_time))` with vote extensions disabled — the same at every height.

Synced values carry the full encoded block in `value_bytes`; Malachite checks the certificate
against the block hash.

## Invariants

1. **Validate before voting.** A proposal is `Valid` only if `ledger::connect_block` succeeds on
   the committed tip.
2. **Persist before replying.** A block we build is stored before the `GetValue` reply, and a
   received block before the `ReceivedProposalPart` reply. After a crash, Malachite replays
   the height and we must return identical answers, or we equivocate.
3. **Commit before acknowledging.** `Decided` is acknowledged only after the redb transaction
   commits (fsync).
4. **Validate once.** The dry-run `BlockDiff` is cached by block hash and applied on commit;
   a cache miss re-runs `connect_block` in the commit path.

## Crash recovery

| Crash point | On restart |
|---|---|
| Before the commit of height H | Start at H; Malachite replays its WAL for H; stored proposals give identical answers |
| After the commit of H, before the `Finalized` reply | Start at H + 1 |

`NodeHandle::shutdown` stops the engine and waits for the app task, releasing the WAL and
database locks; a restarted node catches up through value sync.

## Tests

`crates/node/tests/network.rs`: 4 in-process validators decide ≥ 5 blocks; one stops while 3
continue; it restarts and catches up; every node holds the same block and a certificate at
every height.
