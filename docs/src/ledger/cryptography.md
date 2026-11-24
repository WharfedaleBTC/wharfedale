# Cryptography

All primitives come from libraries. Notation: secp256k1, generator `G`.

| Primitive | Formula / rule | API |
|---|---|---|
| Asset generator | `H_A = hash_to_curve(asset_id)` | `Generator::new_unblinded(Tag)` |
| Asset commitment | `A' = H_A + r_a·G` | `Generator::new_blinded(Tag, r_a)` |
| Value commitment | `C = v·A' + r_v·G` (against the output's own `A'`) | `PedersenCommitment::new(v, r_v, A')` |
| Explicit value | `C = v·H_A`, zero blinding | `PedersenCommitment::new_unblinded(v, H_A)` |
| Balance | Σ C_in + Σ C_issuance = Σ C_out (fees included as explicit outputs) | `verify_commitments_sum_to_equal` |
| Rangeproof | Value in 64-bit range; bound to `script_pubkey`; **min_value ≥ 1 if spendable** | `RangeProof::verify(C, spk, A') → Range<u64>` |
| Surjection proof | Output `A'` is a re-blinding of one domain generator | `SurjectionProof::verify(A'_out, domain)` |
| Issuance entropy | `E = fmr(SHA256d(prevout), contract_hash)` | `AssetId::generate_asset_entropy` |
| Asset id | `fmr(E, 0)` | `AssetId::from_entropy` |
| Reissuance token | `fmr(E, 1)` (explicit amount) or `fmr(E, 2)` (confidential) | `AssetId::reissuance_token_from_entropy` |
| Taproot | `Q = P + H_TapTweak/elements(P ‖ root)·G`; leaves `H_TapLeaf/elements(ver ‖ script)` | `elements::taproot` |
| Signature | BIP340 Schnorr by `Q` | `secp256k1_zkp::schnorr` |
| Sighash | Elements taproot sighash (tag `TapSighash/elements`) | `SighashCache::taproot_key_spend_signature_hash(.., genesis_hash)` |
| ECDH (wallets) | `shared = SHA256d(ECDH(nonce_pk, blinding_sk))` | `confidential::Nonce::shared_secret` |

`fmr` = Elements' fast-merkle-root SHA-256 midstate construction.

## What the sighash commits to

Genesis hash (twice); version and lock time; all spent outpoints with flags; all spent asset
and value commitments; all spent scriptPubKeys; all sequences; all issuances and issuance
rangeproofs; all outputs and **all output witnesses** (proofs); spend type (annex) and input
index. A signed transaction can't have a proof, amount or blinding swapped.

## Surjection domain order

For each input `i` in order: the input's asset generator; then, if it issues, `H_asset`; then,
if it issues tokens, `H_token`. A different order rejects valid proofs.

## Rules that are easy to miss

- An explicit zero value can't be committed, so a spendable explicit zero output is invalid.
- A rangeproof with `min_value == 0` on a spendable output is invalid (blocks zero-value tokens).
  "Unspendable" is Elements' `IsUnspendable`: OP_RETURN, oversized, **or empty** script, so
  issuance proofs (committed to an empty script) may have `min_value == 0`.
- Issuance amounts are committed against the **unblinded** `H_asset` / `H_token`.
- A reissuance input's spent asset commitment must equal `new_blinded(token_id, asset_blinding_nonce)` byte for byte.
- Only a new issuance may create tokens (`inflation_keys`).

## Library rule

Use `elements::secp256k1_zkp` (0.11) only; never add `secp256k1-zkp` as a direct dependency.
