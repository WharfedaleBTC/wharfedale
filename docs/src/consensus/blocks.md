# Blocks and proposals

## Format (`wharfedale-chain`)

```rust
struct Header {                     // 136 bytes, little-endian, fields in this order
    version: u32,                   // 1
    height: u64,
    prev_block_hash: [u8; 32],      // genesis: hash of the genesis parameters
    timestamp: u64,                 // unix seconds
    proposer: [u8; 20],             // consensus address; zero in genesis
    tx_merkle_root: [u8; 32],       // over Elements txids
    witness_merkle_root: [u8; 32],  // over wtxids; a leading coinbase contributes zero
}
struct Block { header: Header, txs: Vec<elements::Transaction> } // a coinbase, if any, is txs[0]
```

| Item | Definition |
|---|---|
| Block hash | SHA-256d(encoded header). Malachite's `Value` and `ValueId` |
| Block encoding | `header ‖ count: u32 ‖ (len: u32 ‖ elements consensus tx)*` (`Block::encode`) |
| Merkle root | Bitcoin construction: pairwise SHA-256d, last node duplicated on odd levels, empty = zero |
| Genesis `prev_block_hash` | SHA-256d(`"wharfedale/genesis"` ‖ chain_id ‖ fee_asset ‖ validators (key ‖ power)) — the genesis hash identifies chain id, fee asset and validator set |

Commit certificates are stored beside their block and served by sync, which verifies them.
They are **not** committed in the next header: each validator's certificate can hold a
different subset of signatures, so a hash of it isn't the same on every node.

Open decision: `state_root` (UTXO multiset hash) — see [Constants](../constants.md).

## Header rules (`check_header`)

| Field | Rule |
|---|---|
| `version` | 1 |
| `height` | Parent height + 1 |
| `prev_block_hash` | Hash of the committed tip |
| `proposer` | The proposer Malachite selected for (height, round) |
| `timestamp` | > parent timestamp; for a proposal also ≤ local time + 10 s (`MAX_TIME_DRIFT`). Not checked for decided blocks received by sync |
| `tx_merkle_root`, `witness_merkle_root` | Recomputed from `txs`, and the txid tree must not be *mutated* (two equal siblings at any level: CVE-2012-2459, where `[a,b,c]` and `[a,b,c,c]` share a root) (`Block::roots_match`) |

The proposer sets `timestamp = max(now, parent + 1)`. Time locks use the median of the last 11
header timestamps (MTP).

## Proposal parts

Mode `ProposalAndParts`: consensus signs `Proposal(Value = block hash)`; the app streams the
block.

```rust
enum ProposalPart {
    Init(ProposalInit { height, round, pol_round, proposer }),
    Data(ProposalData { chunk: Bytes }),   // ≤ 64 KiB slices of Block::encode()
    Fin(ProposalFin { signature }),        // proposer Ed25519 over the parts digest
}
```

- Parts digest = SHA-256d(`"wharfedale/proposal/v2"` ‖ height (u64) ‖ round (u32) ‖ pol_round
  (u32, from `Init`; nil = `u32::MAX`) ‖ (len ‖ chunk)*).
- Stream id = height (u64 BE) ‖ round (u32 BE); messages are `Data(part)` with sequence 0..n,
  then `Fin`. Published with `NetworkMsg::PublishProposalPart`.
- Reassembly groups by publisher and stream id (`node::streaming`, from Malachite `test/streaming`).
  At most 64 streams are assembled at once (oldest dropped) and sequence numbers above 80 are
  ignored (a maximal 4 MB block is ~67 messages).
- A receiver checks: proposer = the selected proposer for (height, round); `Init` matches the
  stream; the signature; the block decodes, has that height and `roots_match`. A failure here
  means a forged or broken stream: the parts are ignored. Then [full validation](app-loop.md).

## No empty blocks

A validator builds a block only when its mempool has something to put in it, and holds the
next height until then (see [Holding a height](app-loop.md#holding-a-height)). An idle chain
does not grow. Validity doesn't require transactions: blocks made before this rule, which
carry only a zero-value coinbase, stay valid; not building empty blocks is the proposer's
policy. `create_empty_blocks = true` in `config.toml` turns empty blocks back on (the M2
liveness test uses it).

## Coinbase

Optional. It exists only to pay the block's fees to the proposer, so a proposer adds one when
its transactions pay fees (every standard transaction does: the mempool's minimum fee rate is
100 per kvB) and leaves it out otherwise; fees in a block without one are burned. When
present it is `txs[0]`:
- one input with a null prevout; `script_sig` = `push_int(height) ‖ push("wharfedale")`
  (2–100 bytes, starts with the height: BIP34, unique txid);
- no issuance, peg-in or input witness;
- explicit outputs only, no output witnesses;
- consensus: per asset, Σ outputs ≤ the block's fee map (subsidy 0), Σ explicit outputs ≤
  `MAX_MONEY`;
- a Wharfedale proposer claims **only the fee asset**, in one output to its
  `fee_payout_script` (a taproot script, checked at startup). Unclaimed fees (other assets,
  which mempool policy refuses anyway) are burned. One output keeps the coinbase far below `COINBASE_WEIGHT_RESERVE` (4 000 WU).
