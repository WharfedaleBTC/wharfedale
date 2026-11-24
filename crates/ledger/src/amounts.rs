//! Stage 4: amounts. Port of Elements `VerifyAmounts`, `VerifyIssuanceAmount` and
//! `CachingRangeProofChecker::VerifyRangeProof`
//! (`src/confidential_validation.cpp:106-416`, `src/script/sigcache.cpp:161-201`).
//!
//! Checks, in the C++ order:
//! 1. Inputs: commitment per spent output; surjection domain = input generators, then for each
//!    issuing input its issued-asset and token generators (`:160-166`).
//! 2. Reissuance: the spent asset commitment equals the token generator blinded with the revealed
//!    nonce (`:226-242`); reissuance can't create tokens (`:263-265`).
//! 3. Issuance amounts: explicit > 0 without proof, or committed with a rangeproof against the
//!    unblinded asset generator and an empty script (`:106-141`).
//! 4. Outputs: commitment per output; explicit zero only on unspendable outputs (`:330-345`).
//! 5. Balance: Σ inputs + issuances = Σ outputs (`secp256k1_pedersen_verify_tally`).
//! 6. Rangeproof per confidential output value, bound to its scriptPubKey; `min_value == 0` is
//!    rejected unless the script is unspendable (`sigcache.cpp:194-201`).
//! 7. Surjection proof per confidential output asset over the domain of step 1.

use elements::confidential::{Asset, Value};
use elements::secp256k1_zkp::{
    verify_commitments_sum_to_equal, Generator, PedersenCommitment, Tweak, SECP256K1,
};
use elements::{AssetId, RangeProof, Script, Transaction};

use crate::consts::MAX_MONEY;
use crate::error::{ProofLoc, TxError};
use crate::types::UtxoEntry;

fn generator(asset: AssetId) -> Generator {
    Generator::new_unblinded(SECP256K1, asset.into_tag())
}

pub(crate) fn verify(tx: &Transaction, spent: &[UtxoEntry]) -> Result<(), TxError> {
    let mut ins: Vec<PedersenCommitment> = Vec::with_capacity(spent.len() + 2);
    let mut outs: Vec<PedersenCommitment> = Vec::with_capacity(tx.output.len());
    let mut domain: Vec<Generator> = Vec::with_capacity(spent.len() + 2);

    // 1–3. Inputs and issuance pseudo-inputs.
    for (i, (input, entry)) in tx.input.iter().zip(spent).enumerate() {
        let prev = &entry.txout;
        let gen = match prev.asset {
            Asset::Explicit(a) => generator(a),
            Asset::Confidential(g) => g,
            Asset::Null => return Err(TxError::SpentValueInvalid(i)),
        };
        domain.push(gen);
        let commit = match prev.value {
            // Elements: MoneyRange; a zero value can't be committed.
            Value::Explicit(v) if v > 0 && v <= MAX_MONEY => PedersenCommitment::new_unblinded(SECP256K1, v, gen),
            Value::Confidential(c) => c,
            _ => return Err(TxError::SpentValueInvalid(i)),
        };
        ins.push(commit);

        let iss = &input.asset_issuance;
        if iss.is_null() {
            continue;
        }
        let (asset_id, token_id) = input.issuance_ids();
        let reissuance = !iss.asset_blinding_nonce.is_null();
        if reissuance {
            let nonce = Tweak::from_slice(iss.asset_blinding_nonce.as_byte_array())
                .map_err(|_| TxError::ReissuanceTokenMismatch(i))?;
            let expected = Generator::new_blinded(SECP256K1, token_id.into_tag(), nonce);
            match prev.asset {
                Asset::Confidential(g) if g == expected => {}
                _ => return Err(TxError::ReissuanceTokenMismatch(i)),
            }
        }
        if !iss.amount.is_null() {
            let (c, g) = issuance_commitment(asset_id, iss.amount, &input.witness.amount_rangeproof, ProofLoc::IssuanceAmount(i))?;
            domain.push(g);
            ins.push(c);
        }
        if !iss.inflation_keys.is_null() {
            if reissuance {
                return Err(TxError::ReissuanceCreatesTokens(i));
            }
            let (c, g) =
                issuance_commitment(token_id, iss.inflation_keys, &input.witness.inflation_keys_rangeproof, ProofLoc::IssuanceTokens(i))?;
            domain.push(g);
            ins.push(c);
        }
    }

    // 4. Outputs.
    let mut out_gens: Vec<Generator> = Vec::with_capacity(tx.output.len());
    for (o, out) in tx.output.iter().enumerate() {
        let gen = match out.asset {
            Asset::Explicit(a) => generator(a),
            Asset::Confidential(g) => g,
            Asset::Null => return Err(TxError::NullAssetOrValue(o)),
        };
        out_gens.push(gen);
        match out.value {
            Value::Explicit(0) if out.script_pubkey.is_provably_unspendable() => {}
            Value::Explicit(0) => return Err(TxError::ZeroValue(o)),
            Value::Explicit(v) if v > MAX_MONEY => return Err(TxError::ValueOutOfRange(o)),
            Value::Explicit(v) => outs.push(PedersenCommitment::new_unblinded(SECP256K1, v, gen)),
            Value::Confidential(c) => outs.push(c),
            Value::Null => return Err(TxError::NullAssetOrValue(o)),
        }
    }

    // 5. Balance.
    if !verify_commitments_sum_to_equal(SECP256K1, &ins, &outs) {
        return Err(TxError::BalanceMismatch);
    }

    // 6. Output rangeproofs.
    for (o, out) in tx.output.iter().enumerate() {
        if let Value::Confidential(c) = out.value {
            verify_rangeproof(&out.witness.rangeproof, c, &out.script_pubkey, out_gens[o], ProofLoc::Output(o))?;
        } else if !out.witness.rangeproof.is_empty() {
            return Err(TxError::UnexpectedOutputProof(o));
        }
    }

    // 7. Surjection proofs.
    for (o, out) in tx.output.iter().enumerate() {
        match out.asset {
            Asset::Confidential(g) => {
                let proof = out.witness.surjection_proof.as_ref().ok_or(TxError::MissingSurjectionProof(o))?;
                if !proof.verify(SECP256K1, g, &domain) {
                    return Err(TxError::BadSurjectionProof(o));
                }
            }
            _ if !out.witness.surjection_proof.is_empty() => return Err(TxError::UnexpectedOutputProof(o)),
            _ => {}
        }
    }
    Ok(())
}

/// `VerifyIssuanceAmount`: the commitment and generator of an issuance pseudo-input.
fn issuance_commitment(
    asset: AssetId,
    value: Value,
    proof: &RangeProof,
    loc: ProofLoc,
) -> Result<(PedersenCommitment, Generator), TxError> {
    let gen = generator(asset);
    match value {
        Value::Explicit(v) => {
            // WHARFEDALE: Elements caps explicit issuance at MAX_MONEY only for the pegged
            // asset (i64::MAX otherwise); every asset is capped here.
            if v == 0 || v > MAX_MONEY {
                return Err(TxError::IssuanceAmountInvalid(loc));
            }
            if !proof.is_empty() {
                return Err(TxError::UnexpectedIssuanceProof(loc.input()));
            }
            Ok((PedersenCommitment::new_unblinded(SECP256K1, v, gen), gen))
        }
        Value::Confidential(c) => {
            // Issuance proofs commit to an empty script, which Elements treats as unspendable.
            verify_rangeproof(proof, c, &Script::new(), gen, loc)?;
            Ok((c, gen))
        }
        Value::Null => Err(TxError::IssuanceAmountInvalid(loc)),
    }
}

/// `CachingRangeProofChecker::VerifyRangeProof` (without the cache).
fn verify_rangeproof(
    proof: &RangeProof,
    commitment: PedersenCommitment,
    spk: &Script,
    gen: Generator,
    loc: ProofLoc,
) -> Result<(), TxError> {
    let proof = proof.as_ref().ok_or(TxError::MissingRangeproof(loc))?;
    let range = proof.verify(SECP256K1, commitment, spk.as_bytes(), gen).map_err(|_| TxError::BadRangeproof(loc))?;
    // No circulating zero-value outputs (blocks conjuring zero-value reissuance tokens).
    if range.start == 0 && !spk.is_provably_unspendable() {
        return Err(TxError::RangeproofMinValueZero(loc));
    }
    Ok(())
}
