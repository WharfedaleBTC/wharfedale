# Simplicity

Programs run in the C library's Elements environment against the real transaction, so every
jet has its Liquid meaning. Code: `crates/ledger/src/spend.rs` (rules) →
`crates/ledger-simplicity-sys` (FFI) → C `execSimplicity`.

## Witness layout (taproot script path)

`[simplicity_witness, simplicity_program, cmr (32 bytes), control_block]` (+ optional annex,
the last element starting with `0x50`).

| Check | Rule | Error |
|---|---|---|
| Leaf version | `control_block[0] & 0xfe == 0xbe` | `LeafVersionForbidden` |
| Shape | Exactly 4 elements without the annex; `cmr` is 32 bytes | `WitnessShape` |
| Control block | `33 + 32·m` bytes (`m ≤ 128`), valid internal key | `ControlBlock` |
| Commitment | `tapleaf_hash(0xbe, cmr)` + merkle path in the control block yields `Q` | `TaprootCommitment` |
| Budget | `serialized_size(witness stack incl. annex) + 50` weight units (Elements `VALIDATION_WEIGHT_OFFSET`) | |
| Execution | `execSimplicity(err, NULL, tx, ix, tapEnv, genesis_hash, 0, budget, NULL, program, len, witness, len)` returns true and `err == NO_ERROR` | `SimplicityRejected(ix, name)` |

Inside the C library: decode the program, check its CMR equals `cmr`, type-check, run static
cost analysis against the budget, then execute.

**Error mapping** (as Elements `CheckSimplicity`): `NO_ERROR` → valid. `MALLOC`,
`NOT_YET_IMPLEMENTED`, or the call returning `false` → `SimplicityNodeFault`: a local failure,
not remembered as invalid; a proposal or synced block hitting it gets no verdict (the node prevotes nil after the proposal timeout; sync retries), and the mempool keeps such a transaction. Every other code (e.g.
`CMR`, `EXEC_JET`, `EXEC_BUDGET`) → `SimplicityRejected`: the input is invalid.

Results are not cached: every check executes the program again.

## `wharfedale-ledger-simplicity-sys`

The only crate with `unsafe`. `simplicity-sys` 0.8 with feature `test-utils` (which compiles
the decoder and evaluator); C symbols are prefixed `rustsimplicity_0_8_`.

| Item | Does |
|---|---|
| `CTx::new(tx, spent: &[TxOut])` | Raw C transaction (adapted from rust-simplicity `c_env.rs`), `mallocTransaction`; freed on drop. Built once per transaction, on the first Simplicity input |
| `exec(&CTx, ix, control_block, cmr, genesis, program, witness, budget)` | `mallocTapEnv`, `execSimplicity`, `freeTapEnv`; `Result<(), ExecError>` |
| `ExecError::Rejected { code, name }` / `NodeFault(&str)` | `simplicity_err` code and name / local failure |
| `taproot::{script_pubkey, control_block, spend_witness}(cmr)` | Contract outputs (below) |

## Contract outputs

`OP_1 <Q>` with a single leaf `(script = cmr, version 0xbe)` under the BIP341 NUMS internal key
`50929b74…803ac0` (no key path). Explicit address `wf1p…`. Control block: 33 bytes.

## Environment values

| Jet / value | Wharfedale |
|---|---|
| `genesis_block_hash` | Wharfedale genesis hash (replay protection via `sig_all_hash`) |
| `lbtc_asset` | Liquid's constant L-BTC id. **Meaningless here: contracts must not use it.** |
| Peg-in jets | Always "no pegin" |
| Lock jets (`check_lock_*`, `tx_lock_*`) | Read tx fields; meaningful because consensus enforces nLockTime and BIP68. Time locks are enforced against the block's timestamp (not MTP), so `check_lock_time(t)` means "in a block stamped after `t`" |
| Everything else | Identical to Liquid |

There is no current-height jet, so a Simplicity result depends only on the transaction, the
spent outputs and the genesis hash.

## Tooling

Contracts: SimplicityHL 0.9 (`simplicityhl`, Elements jets). The wallet compiles, satisfies and
prunes with it (`satisfy_with_env` against rust-simplicity's `ElementsEnv`); consensus never
uses it. Examples: `examples/contracts/{p2pk,timelock}.simf`. See [Wallet](../wallet.md).
