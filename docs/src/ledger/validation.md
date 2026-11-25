# Validation

## `check_tx` pipeline (cheapest first; the first failure stops)

| # | Stage | Checks | Elements source |
|---|---|---|---|
| 1 | Stateless | ≥ 1 input and output; weight ≤ `MAX_TX_WEIGHT`; no duplicate prevouts; version 2; no pegin; empty `scriptSig`; empty pegin witness; allowed output kinds; valid encodings; explicit values in `0 < v ≤ MAX_MONEY` (zero only for `OP_RETURN`); fee outputs explicit and > 0; explicit value ⇒ no rangeproof; explicit asset ⇒ no surjection proof | `CheckTransaction`, `HasValidFee` |
| 2 | Inputs | Every prevout in the `LedgerView`; coinbase outputs mature | `Consensus::CheckTxInputs` |
| 3 | Locks | nLockTime (height, or MTP for times) unless all sequences final; BIP68 relative locks | `IsFinalTx`, `SequenceLocks` |
| 4 | Amounts | Build input commitments + issuance pseudo-inputs + surjection domain; reissuance token check; build output commitments; balance; rangeproofs; surjection proofs | `VerifyAmounts`, `VerifyIssuanceAmount`, `CachingRangeProofChecker::VerifyRangeProof` |
| 5 | Spends | Per input: taproot key path or Simplicity ([Simplicity](simplicity.md)) | `VerifyWitnessProgram` (taproot branch) |
| 6 | Fees | Return Σ explicit fee outputs per asset | `GetFeeMap` |

Stages 4 and 5 run on every check: no verification result is cached.
Their results depend only on the transaction, the spent outputs and the genesis hash, so they
stay valid across blocks. Parallel verification across inputs and outputs is planned (M6).

## Amounts (stage 4, `amounts.rs`)

Order and rules exactly as Elements `VerifyAmounts`:

| Step | Rule | Error |
|---|---|---|
| Inputs | Spent asset not null; explicit value in 1..=MAX_MONEY (zero can't be committed) | `SpentValueInvalid` |
| Domain | Per input: its generator, then `H_asset` if it issues an amount, then `H_token` if it issues tokens | — |
| Reissuance | Spent asset commitment = `Generator::new_blinded(token_id, asset_blinding_nonce)` | `ReissuanceTokenMismatch` |
| Reissuance | No `inflation_keys` | `ReissuanceCreatesTokens` |
| Issuance amounts | Explicit: 1..=MAX_MONEY, no proof. Committed: rangeproof against the unblinded generator, empty script | `IssuanceAmountInvalid`, `UnexpectedIssuanceProof`, `*Rangeproof` |
| Outputs | Explicit zero only if `is_provably_unspendable` (OP_RETURN, oversized, or empty script — Elements rule) | `ZeroValue` |
| Balance | `verify_commitments_sum_to_equal(inputs + issuances, outputs)` | `BalanceMismatch` |
| Rangeproofs | Every committed output value; extra commitment = scriptPubKey; `min_value == 0` only on unspendable scripts | `MissingRangeproof`, `BadRangeproof`, `RangeproofMinValueZero` |
| Surjection | Every committed output asset, over the domain | `MissingSurjectionProof`, `BadSurjectionProof` |

## Spend authorization (stage 5)

```text
spk = spent.script_pubkey;  require spk == OP_1 <Q>
stack = witness.script_witness; if len ≥ 2 and last starts with 0x50: annex = pop()
len == 1 → BIP340(Q, taproot_key_spend_signature_hash(ix, prevouts, type, genesis), stack[0])
len == 4 → [witness, program, cmr(32), control_block]:
           leaf_version(control_block) == 0xbe
           control block commits tapleaf(0xbe, cmr) to Q
           simplicity_exec(.., budget = serialized_size(stack) + 50)
otherwise → reject (Tapscript and unknown shapes)
```

## `connect_block`

1. If `txs[0]` is a coinbase: one null-prevout input, `script_sig = push(height)`, explicit
   outputs, no output witnesses, spendable outputs > 0. A coinbase anywhere else is rejected.
2. For each other transaction: `check_tx` against the view plus earlier outputs in the block;
   record spent and created entries; add to the fee map.
3. Coinbase: for each asset, Σ outputs ≤ `fees[asset]` (`VerifyCoinbaseAmount`, subsidy 0).
   Without a coinbase the fees are burned.
4. Block weight ≤ `MAX_BLOCK_WEIGHT`. This also bounds Simplicity work: each input's budget is
   its own witness size.
5. Return `BlockDiff`. Fee and `OP_RETURN` outputs are not created in the UTXO set.

## Deviations from Elements (all stricter, marked `// WHARFEDALE:`)

| Rule | Elements | Wharfedale |
|---|---|---|
| Output kinds | Any script | Taproot, fee, `OP_RETURN` only |
| `OP_RETURN` amounts | Explicit or confidential | Explicit only |
| Issuance rangeproof next to a null or explicit amount | Ignored (null) / rejected (explicit) | Rejected |
| Taproot leaf versions other than `0xbe` | Tapscript runs; unknown versions succeed | Rejected |
| Fee outputs | Stored in the coin set (unspendable) | Not stored |
| Transaction version | Any | 2 only |
| Explicit issuance amount | ≤ `MAX_MONEY` for the pegged asset, ≤ `i64::MAX` otherwise | ≤ `MAX_MONEY` for every asset |
| Transaction weight | Stripped size × 4 ≤ `MAX_BLOCK_WEIGHT` | Full weight ≤ `MAX_TX_WEIGHT` (400 000) |
| Empty-script output with a confidential value | Unspendable output | Rejected (an empty script is a fee output, which must be explicit) |

One deviation is looser: the coinbase is optional (Elements requires one in every block). It
only pays fees, so a block whose transactions pay none needs none, and an empty block (no
transactions) is structurally valid; proposers don't build those (see
[No empty blocks](../consensus/blocks.md#no-empty-blocks)).

## Porting rules

- Each Rust function cites the Elements C++ function and lines it mirrors.
- Same check order and edge cases. Wharfedale's deviations are exactly the ones in the
  [subset table](README.md) and are marked `// WHARFEDALE:` in code.
- Do not use `Transaction::verify_tx_amt_proofs` from rust-elements: it skips issuances.
- Every rule has a negative test (unit tests in `crates/ledger/tests/`, end to end in
  `crates/node/tests/` and Docker).
