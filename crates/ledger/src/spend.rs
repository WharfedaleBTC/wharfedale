//! Stage 5: spend authorization. Taproot only: key path (BIP340) or Simplicity leaf `0xbe`.
//!
//! Mirrors the taproot branch of Elements `VerifyWitnessProgram`
//! (`src/script/interpreter.cpp`, witness v1). No Script interpreter exists here.

use elements::encode::serialize;
use elements::schnorr::{SchnorrSig, TweakedPublicKey};
use elements::secp256k1_zkp::{Message, XOnlyPublicKey, SECP256K1};
use elements::sighash::{Annex, Prevouts, SighashCache};
use elements::taproot::ControlBlock;
use elements::{BlockHash, Script, Transaction, TxOut};
use wharfedale_ledger_simplicity_sys::{self as simplicity, CTx, ExecError};

use crate::consts::*;
use crate::error::TxError;
use crate::types::{ChainCtx, UtxoEntry};

pub(crate) fn verify(
    tx: &Transaction,
    spent: &[UtxoEntry],
    ctx: &ChainCtx,
) -> Result<(), TxError> {
    let prevouts: Vec<TxOut> = spent.iter().map(|e| e.txout.clone()).collect();
    let mut env = Env { tx, prevouts: &prevouts, sighashes: SighashCache::new(tx), c_tx: None, genesis_hash: ctx.genesis_hash };
    for ix in 0..tx.input.len() {
        verify_input(&mut env, ix)?;
    }
    Ok(())
}

/// Per-transaction data shared by its inputs; the C transaction is built on first use.
struct Env<'a> {
    tx: &'a Transaction,
    prevouts: &'a [TxOut],
    sighashes: SighashCache<&'a Transaction>,
    c_tx: Option<CTx>,
    genesis_hash: BlockHash,
}

fn verify_input(env: &mut Env, ix: usize) -> Result<(), TxError> {
    let (tx, prevouts) = (env.tx, env.prevouts);
    let spk = &prevouts[ix].script_pubkey;
    if !spk.is_v1_p2tr() {
        return Err(TxError::SpentNotTaproot(ix));
    }
    let output_key = XOnlyPublicKey::from_slice(&spk.as_bytes()[2..34])
        .map_err(|_| TxError::SpentNotTaproot(ix))?;

    let mut stack: Vec<&[u8]> = tx.input[ix].witness.script_witness.iter().collect();
    if stack.is_empty() {
        return Err(TxError::WitnessEmpty(ix));
    }
    // BIP341: with ≥ 2 elements, a last element starting with 0x50 is the annex.
    let annex = match stack.last() {
        Some(last) if stack.len() >= 2 && last.first() == Some(&ANNEX_TAG) => {
            let bytes = stack.pop().expect("non-empty");
            Some(Annex::new(bytes).expect("starts with 0x50"))
        }
        _ => None,
    };

    if stack.len() == 1 {
        return verify_key_path(ix, prevouts, &mut env.sighashes, env.genesis_hash, &output_key, stack[0], annex);
    }

    // Script path: the last element is the control block.
    let control = stack.last().expect("len >= 2");
    let leaf_version = control.first().ok_or(TxError::ControlBlock(ix))? & TAPROOT_LEAF_MASK;
    if leaf_version != TAPROOT_LEAF_SIMPLICITY {
        // WHARFEDALE: Tapscript (0xc4) and unknown leaf versions are invalid, not upgrade hooks.
        return Err(TxError::LeafVersionForbidden(ix, leaf_version));
    }
    // Elements `SCRIPT_ERR_SIMPLICITY_WRONG_LENGTH`: [witness, program, cmr, control block].
    let [witness, program, cmr, control] = stack[..] else {
        return Err(TxError::WitnessShape(ix));
    };
    let cmr: &[u8; 32] = cmr.try_into().map_err(|_| TxError::WitnessShape(ix))?;
    // Size (33 + 32·m, m ≤ 128), leaf version parity, internal key; then the BIP341 commitment
    // to the leaf script (the CMR) under the spent output key.
    let cb = ControlBlock::from_slice(control).map_err(|_| TxError::ControlBlock(ix))?;
    let script = Script::from(cmr.to_vec());
    if !cb.verify_taproot_commitment(SECP256K1, &TweakedPublicKey::new(output_key), &script) {
        return Err(TxError::TaprootCommitment(ix));
    }
    // Elements: budget = serialized size of the whole witness stack (annex included) + 50.
    let budget = serialize(&tx.input[ix].witness.script_witness).len() as i64 + VALIDATION_WEIGHT_OFFSET;
    if env.c_tx.is_none() {
        env.c_tx = Some(CTx::new(tx, prevouts).map_err(|e| exec_error(ix, e))?);
    }
    let c_tx = env.c_tx.as_ref().expect("built above");
    simplicity::exec(c_tx, ix as u32, control, cmr, env.genesis_hash.as_ref(), program, witness, budget)
        .map_err(|e| exec_error(ix, e))
}

fn exec_error(ix: usize, e: ExecError) -> TxError {
    match e {
        ExecError::Rejected { name, .. } => TxError::SimplicityRejected(ix, name),
        ExecError::NodeFault(what) => TxError::SimplicityNodeFault(ix, what),
    }
}

fn verify_key_path(
    ix: usize,
    prevouts: &[TxOut],
    sighashes: &mut SighashCache<&Transaction>,
    genesis_hash: BlockHash,
    output_key: &XOnlyPublicKey,
    raw_sig: &[u8],
    annex: Option<Annex>,
) -> Result<(), TxError> {
    // BIP341: 64 bytes (SIGHASH_DEFAULT) or 65 bytes with an explicit, non-zero hash type.
    match raw_sig.len() {
        64 => {}
        65 if raw_sig[64] != 0x00 => {}
        _ => return Err(TxError::SignatureEncoding(ix)),
    }
    let sig = SchnorrSig::from_slice(raw_sig).map_err(|_| TxError::SignatureEncoding(ix))?;
    let sighash = sighashes
        .taproot_sighash(ix, &Prevouts::All(prevouts), annex, None, sig.hash_ty, genesis_hash)
        .map_err(|e| TxError::Sighash(ix, e.to_string()))?;
    let msg = Message::from_digest(sighash.to_byte_array());
    SECP256K1
        .verify_schnorr(&sig.sig, &msg, output_key)
        .map_err(|_| TxError::BadSignature(ix))
}
