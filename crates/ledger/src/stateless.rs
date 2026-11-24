//! Stage 1: context-free checks.
//!
//! Mirrors Elements `CheckTransaction` and `HasValidFee` (`src/consensus/tx_check.cpp`,
//! `src/confidential_validation.cpp:33-49`), restricted to the Wharfedale subset.

use std::collections::HashSet;

use elements::script::Builder;
use elements::{confidential, Transaction, TxIn, TxOut};

use crate::consts::*;
use crate::error::{BlockError, TxError};
use crate::types::FeeMap;

/// The output kinds Wharfedale allows (`docs/src/ledger/README.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputKind {
    /// `OP_1 <32 bytes>`: spendable by key path or Simplicity.
    Taproot,
    /// Empty scriptPubKey: explicit fee, claimed by the coinbase.
    Fee,
    /// `OP_RETURN …`: unspendable burn or data.
    OpReturn,
}

/// Classifies an output script. `None` = forbidden.
pub fn output_kind(out: &TxOut) -> Option<OutputKind> {
    let spk = &out.script_pubkey;
    if spk.is_v1_p2tr() {
        Some(OutputKind::Taproot)
    } else if spk.is_empty() {
        Some(OutputKind::Fee)
    } else if spk.is_op_return() {
        Some(OutputKind::OpReturn)
    } else {
        None
    }
}

/// Context-free checks of a non-coinbase transaction. Returns its fee map.
pub(crate) fn check(tx: &Transaction) -> Result<FeeMap, TxError> {
    if tx.input.is_empty() {
        return Err(TxError::NoInputs);
    }
    if tx.output.is_empty() {
        return Err(TxError::NoOutputs);
    }
    if tx.version != TX_VERSION {
        return Err(TxError::Version(tx.version));
    }
    let weight = tx.weight();
    if weight > MAX_TX_WEIGHT {
        return Err(TxError::TooHeavy(weight));
    }

    let mut seen = HashSet::with_capacity(tx.input.len());
    for (i, input) in tx.input.iter().enumerate() {
        if input.previous_output.is_null() {
            return Err(TxError::NullPrevout(i));
        }
        if !seen.insert(input.previous_output) {
            return Err(TxError::DuplicateInput(i));
        }
        check_input(i, input)?;
    }

    check_outputs(tx)
}

fn check_input(i: usize, input: &TxIn) -> Result<(), TxError> {
    if input.is_pegin {
        return Err(TxError::Pegin(i));
    }
    if !input.script_sig.is_empty() {
        return Err(TxError::ScriptSigNotEmpty(i));
    }
    if !input.witness.pegin_witness.is_empty() {
        return Err(TxError::PeginWitness(i));
    }
    // WHARFEDALE: an issuance rangeproof is only allowed next to a confidential amount
    // (Elements ignores stray proofs on null amounts).
    let iss = &input.asset_issuance;
    if (!iss.amount.is_confidential() && !input.witness.amount_rangeproof.is_empty())
        || (!iss.inflation_keys.is_confidential() && !input.witness.inflation_keys_rangeproof.is_empty())
    {
        return Err(TxError::UnexpectedIssuanceProof(i));
    }
    Ok(())
}

fn check_outputs(tx: &Transaction) -> Result<FeeMap, TxError> {
    let mut fees = FeeMap::new();
    // `CheckTransaction`: Σ explicit output values (all assets, all kinds) ≤ MAX_MONEY.
    let mut explicit_total: u64 = 0;
    for (o, out) in tx.output.iter().enumerate() {
        if out.asset.is_null() || out.value.is_null() {
            return Err(TxError::NullAssetOrValue(o));
        }
        if out.value.is_explicit() {
            explicit_total += explicit_value(o, out)?; // each ≤ MAX_MONEY, total checked: no overflow
            if explicit_total > MAX_MONEY {
                return Err(TxError::OutputTotalOutOfRange);
            }
        }
        let kind = output_kind(out).ok_or(TxError::OutputScriptForbidden(o))?;
        match kind {
            OutputKind::Taproot => {
                // Confidential asset and/or value allowed; proofs are checked in stage 4.
                if out.value.is_explicit() && explicit_value(o, out)? == 0 {
                    return Err(TxError::ZeroValue(o));
                }
                if (out.value.is_explicit() && !out.witness.rangeproof.is_empty())
                    || (out.asset.is_explicit() && !out.witness.surjection_proof.is_empty())
                {
                    return Err(TxError::UnexpectedOutputProof(o));
                }
                continue;
            }
            OutputKind::Fee => {
                let (asset, value) = explicit_pair(o, out)?;
                if value == 0 {
                    return Err(TxError::ZeroValue(o));
                }
                let total = fees.entry(asset).or_insert(0);
                *total += value; // each value <= MAX_MONEY and total checked below: no overflow
                if *total > MAX_MONEY {
                    return Err(TxError::FeeOutOfRange(asset));
                }
            }
            OutputKind::OpReturn => {
                // WHARFEDALE: burns must be explicit so supply stays auditable.
                explicit_pair(o, out)?;
            }
        }
        // Explicit asset ⇒ no surjection proof; explicit value ⇒ no rangeproof.
        if !out.witness.is_empty() {
            return Err(TxError::UnexpectedOutputProof(o));
        }
    }
    Ok(fees)
}

fn explicit_value(o: usize, out: &TxOut) -> Result<u64, TxError> {
    match out.value {
        confidential::Value::Explicit(v) if v <= MAX_MONEY => Ok(v),
        confidential::Value::Explicit(_) => Err(TxError::ValueOutOfRange(o)),
        _ => Err(TxError::NotExplicit(o)),
    }
}

fn explicit_pair(o: usize, out: &TxOut) -> Result<(elements::AssetId, u64), TxError> {
    let asset = out.asset.explicit().ok_or(TxError::NotExplicit(o))?;
    Ok((asset, explicit_value(o, out)?))
}

/// The `script_sig` prefix a coinbase at `height` must carry (BIP34).
pub fn coinbase_height_prefix(height: u64) -> Vec<u8> {
    Builder::new().push_int(height as i64).into_script().into_bytes()
}

/// Context-free checks of the coinbase at `height`.
pub(crate) fn check_coinbase(tx: &Transaction, height: u64) -> Result<(), BlockError> {
    if !tx.is_coinbase() {
        return Err(BlockError::Coinbase("first transaction is not a coinbase"));
    }
    if tx.version != TX_VERSION {
        return Err(BlockError::Coinbase("version"));
    }
    if tx.weight() > MAX_TX_WEIGHT {
        return Err(BlockError::Coinbase("too heavy"));
    }
    let input = &tx.input[0];
    if input.is_pegin || !input.asset_issuance.is_null() || !input.witness.is_empty() {
        return Err(BlockError::Coinbase("input must carry no pegin, issuance or witness"));
    }
    let sig = input.script_sig.as_bytes();
    if !(COINBASE_SCRIPT_SIG_MIN..=COINBASE_SCRIPT_SIG_MAX).contains(&sig.len()) {
        return Err(BlockError::Coinbase("script_sig length"));
    }
    if !sig.starts_with(&coinbase_height_prefix(height)) {
        return Err(BlockError::Coinbase("script_sig must start with the block height (BIP34)"));
    }
    if tx.output.is_empty() {
        return Err(BlockError::Coinbase("no outputs"));
    }
    let mut explicit_total: u64 = 0;
    for out in &tx.output {
        let (Some(_), confidential::Value::Explicit(value)) = (out.asset.explicit(), out.value)
        else {
            return Err(BlockError::Coinbase("outputs must be explicit"));
        };
        if value > MAX_MONEY {
            return Err(BlockError::Coinbase("output value out of range"));
        }
        explicit_total += value;
        if explicit_total > MAX_MONEY {
            return Err(BlockError::Coinbase("output total out of range"));
        }
        if !out.witness.is_empty() {
            return Err(BlockError::Coinbase("outputs must carry no witness"));
        }
        match output_kind(out) {
            Some(OutputKind::Taproot) if value > 0 => {}
            Some(OutputKind::Taproot) => {
                return Err(BlockError::Coinbase("zero-value spendable output"))
            }
            Some(OutputKind::OpReturn) => {}
            _ => return Err(BlockError::Coinbase("output kind must be taproot or OP_RETURN")),
        }
    }
    Ok(())
}
