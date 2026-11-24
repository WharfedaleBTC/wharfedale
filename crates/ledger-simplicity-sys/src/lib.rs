//! Thin FFI over the Simplicity C library's Elements consensus entry point,
//! `simplicity_elements_execSimplicity` (as called by Elements `CheckSimplicity`,
//! `src/script/interpreter.cpp:3141`), plus helpers for Simplicity-only taproot outputs.
//!
//! The C library is built by `simplicity-sys` 0.8 (symbols prefixed `rustsimplicity_0_8_`);
//! its `test-utils` feature is what compiles the decoder/evaluator (`exec.c`). The raw
//! transaction construction is adapted from rust-simplicity `src/jet/elements/c_env.rs` (CC0).
//!
//! The only crate in the workspace allowed `unsafe`. Spec: `docs/src/ledger/simplicity.md`.

use std::os::raw::{c_int, c_uchar};
use std::ptr::NonNull;

use elements::encode::serialize;
use elements::{confidential, Transaction, TxOut};
use simplicity_sys::c_jets::c_env::elements as c;
use simplicity_sys::ffi::c_uint_fast32_t;

pub mod taproot;

extern "C" {
    #[link_name = "rustsimplicity_0_8_elements_execSimplicity"]
    fn exec_simplicity(
        error: *mut c_int,
        ihr: *mut c_uchar,
        tx: *const c::CTransaction,
        ix: c_uint_fast32_t,
        taproot: *const c::CTapEnv,
        genesis_block_hash: *const c_uchar,
        min_cost: i64,
        budget: i64,
        amr: *const c_uchar,
        program: *const c_uchar,
        program_len: usize,
        witness: *const c_uchar,
        witness_len: usize,
    ) -> bool;

    #[link_name = "rustsimplicity_0_8_elements_freeTransaction"]
    fn free_transaction(tx: *mut c::CTransaction);

    #[link_name = "rustsimplicity_0_8_elements_freeTapEnv"]
    fn free_tap_env(env: *mut c::CTapEnv);
}

/// `simplicity_err` codes (`include/simplicity/errorCodes.h`).
pub const NO_ERROR: i32 = 0;
pub const ERR_MALLOC: i32 = -1;
pub const ERR_NOT_YET_IMPLEMENTED: i32 = -3;

/// Name of a `simplicity_err` code.
pub fn error_name(code: i32) -> &'static str {
    match code {
        0 => "NO_ERROR",
        -1 => "MALLOC",
        -2 => "DATA_OUT_OF_RANGE",
        -3 => "NOT_YET_IMPLEMENTED",
        -4 => "DATA_OUT_OF_ORDER",
        -6 => "FAIL_CODE",
        -8 => "RESERVED_CODE",
        -10 => "HIDDEN",
        -12 => "BITSTREAM_EOF",
        -14 => "BITSTREAM_TRAILING_BYTES",
        -16 => "BITSTREAM_ILLEGAL_PADDING",
        -18 => "TYPE_INFERENCE_UNIFICATION",
        -20 => "TYPE_INFERENCE_OCCURS_CHECK",
        -22 => "TYPE_INFERENCE_NOT_PROGRAM",
        -24 => "WITNESS_EOF",
        -26 => "WITNESS_TRAILING_BYTES",
        -28 => "WITNESS_ILLEGAL_PADDING",
        -30 => "UNSHARED_SUBEXPRESSION",
        -32 => "CMR",
        -34 => "EXEC_BUDGET",
        -36 => "EXEC_MEMORY",
        -38 => "EXEC_JET",
        -40 => "EXEC_ASSERT",
        -42 => "ANTIDOS",
        -44 => "HIDDEN_ROOT",
        -46 => "AMR",
        -48 => "OVERWEIGHT",
        _ => "UNKNOWN",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecError {
    /// The program is invalid or failed (`code` is a `simplicity_err`).
    Rejected { code: i32, name: &'static str },
    /// The node could not evaluate (allocation failure, unimplemented jet, internal error).
    /// Not evidence that the transaction is invalid.
    NodeFault(&'static str),
}

/// The transaction as the C library sees it. Built once per transaction, shared by its inputs.
pub struct CTx(NonNull<c::CTransaction>);

impl Drop for CTx {
    fn drop(&mut self) {
        // SAFETY: allocated by `elements_mallocTransaction`, freed exactly once.
        unsafe { free_transaction(self.0.as_ptr()) }
    }
}

struct RawInput {
    annex: Option<Vec<u8>>,
    issuance_amount: Vec<u8>,
    issuance_inflation_keys: Vec<u8>,
    amount_range_proof: Vec<u8>,
    inflation_keys_range_proof: Vec<u8>,
    asset: Option<[u8; 33]>,
    value: Vec<u8>,
}

struct RawOutput {
    asset: Option<[u8; 33]>,
    value: Vec<u8>,
    nonce: Option<[u8; 33]>,
    surjection_proof: Vec<u8>,
    range_proof: Vec<u8>,
}

fn asset_array(a: &confidential::Asset) -> Option<[u8; 33]> {
    (!a.is_null()).then(|| serialize(a).try_into().expect("non-null asset is 33 bytes"))
}

fn nonce_array(n: &confidential::Nonce) -> Option<[u8; 33]> {
    (!n.is_null()).then(|| serialize(n).try_into().expect("non-null nonce is 33 bytes"))
}

fn value_ptr(v: confidential::Value, data: &[u8]) -> *const c_uchar {
    if v.is_null() {
        std::ptr::null()
    } else {
        data.as_ptr()
    }
}

/// Annex data (after the 0x50 tag), when the witness has ≥ 2 elements and the last starts
/// with 0x50 (BIP341).
fn annex(witness: &elements::Witness) -> Option<&[u8]> {
    let n = witness.iter().count();
    let last = witness.last()?;
    (n >= 2 && last.first() == Some(&0x50)).then(|| &last[1..])
}

impl CTx {
    /// `spent[i]` is the output spent by input `i`.
    pub fn new(tx: &Transaction, spent: &[TxOut]) -> Result<Self, ExecError> {
        assert_eq!(tx.input.len(), spent.len(), "one spent output per input");
        let inputs_data: Vec<RawInput> = tx
            .input
            .iter()
            .zip(spent)
            .map(|(i, prev)| RawInput {
                annex: annex(&i.witness.script_witness).map(<[u8]>::to_vec),
                issuance_amount: serialize(&i.asset_issuance.amount),
                issuance_inflation_keys: serialize(&i.asset_issuance.inflation_keys),
                amount_range_proof: i.witness.amount_rangeproof.to_vec(),
                inflation_keys_range_proof: i.witness.inflation_keys_rangeproof.to_vec(),
                asset: asset_array(&prev.asset),
                value: serialize(&prev.value),
            })
            .collect();
        let outputs_data: Vec<RawOutput> = tx
            .output
            .iter()
            .map(|o| RawOutput {
                asset: asset_array(&o.asset),
                value: serialize(&o.value),
                nonce: nonce_array(&o.nonce),
                surjection_proof: o.witness.surjection_proof.to_vec(),
                range_proof: o.witness.rangeproof.to_vec(),
            })
            .collect();
        // Boxed slice: must not move or reallocate while raw inputs point into it.
        let annexes: Box<[Option<c::CRawBuffer>]> =
            inputs_data.iter().map(|d| d.annex.as_deref().map(c::CRawBuffer::new)).collect();

        let raw_inputs: Vec<c::CRawInput> = tx
            .input
            .iter()
            .zip(spent)
            .zip(&inputs_data)
            .zip(annexes.iter())
            .map(|(((i, prev), d), annex)| c::CRawInput {
                annex: annex.as_ref().map_or(std::ptr::null(), |b| b as *const _),
                prev_txid: i.previous_output.txid.as_ref(),
                pegin: None, // Wharfedale: peg-ins are invalid before this point
                issuance: if i.has_issuance() {
                    c::CRawInputIssuance {
                        blinding_nonce: Some(i.asset_issuance.asset_blinding_nonce.as_byte_array()),
                        asset_entropy: Some(i.asset_issuance.asset_entropy.as_byte_array()),
                        amount: value_ptr(i.asset_issuance.amount, &d.issuance_amount),
                        inflation_keys: value_ptr(i.asset_issuance.inflation_keys, &d.issuance_inflation_keys),
                        amount_range_proof: c::CRawBuffer::new(&d.amount_range_proof),
                        inflation_keys_range_proof: c::CRawBuffer::new(&d.inflation_keys_range_proof),
                    }
                } else {
                    c::CRawInputIssuance::no_issuance()
                },
                txo: c::CRawInputTxo {
                    asset: d.asset.as_ref(),
                    value: value_ptr(prev.value, &d.value),
                    script_pubkey: c::CRawBuffer::new(prev.script_pubkey.as_bytes()),
                },
                script_sig: c::CRawBuffer::new(i.script_sig.as_bytes()),
                prev_txout_index: i.previous_output.vout,
                sequence: i.sequence.to_consensus_u32(),
            })
            .collect();
        let raw_outputs: Vec<c::CRawOutput> = tx
            .output
            .iter()
            .zip(&outputs_data)
            .map(|(o, d)| c::CRawOutput {
                asset: d.asset.as_ref(),
                value: value_ptr(o.value, &d.value),
                nonce: d.nonce.as_ref(),
                script_pubkey: c::CRawBuffer::new(o.script_pubkey.as_bytes()),
                surjection_proof: c::CRawBuffer::new(&d.surjection_proof),
                range_proof: c::CRawBuffer::new(&d.range_proof),
            })
            .collect();
        let txid = tx.txid();
        let raw = c::CRawTransaction {
            txid: txid.as_ref(),
            inputs: raw_inputs.as_ptr(),
            outputs: raw_outputs.as_ptr(),
            n_inputs: raw_inputs.len().try_into().map_err(|_| ExecError::NodeFault("too many inputs"))?,
            n_outputs: raw_outputs.len().try_into().map_err(|_| ExecError::NodeFault("too many outputs"))?,
            version: tx.version,
            locktime: tx.lock_time.to_consensus_u32(),
        };
        // SAFETY: every pointer in `raw` refers to data alive until the end of this function;
        // the C side copies what it keeps.
        let ptr = unsafe { c::simplicity_mallocTransaction(&raw) };
        drop(raw_inputs);
        drop(annexes);
        NonNull::new(ptr).map(Self).ok_or(ExecError::NodeFault("malloc transaction"))
    }
}

/// Runs a Simplicity program for input `ix`: decode, check the CMR against `cmr`, type-check,
/// static cost analysis against `budget` (weight units), execute.
#[allow(clippy::too_many_arguments)]
pub fn exec(
    tx: &CTx,
    ix: u32,
    control_block: &[u8],
    cmr: &[u8; 32],
    genesis_hash: &[u8; 32],
    program: &[u8],
    witness: &[u8],
    budget: i64,
) -> Result<(), ExecError> {
    if control_block.len() < 33 || !(control_block.len() - 33).is_multiple_of(32) {
        return Err(ExecError::Rejected { code: -2, name: "control block size" });
    }
    let raw_tap = c::CRawTapEnv {
        control_block: control_block.as_ptr(),
        script_cmr: cmr.as_ptr(),
        branch_len: ((control_block.len() - 33) / 32).try_into().map_err(|_| ExecError::Rejected { code: -2, name: "control block size" })?,
    };
    // SAFETY: `raw_tap` points to live data for the duration of the call.
    let tap = NonNull::new(unsafe { c::simplicity_mallocTapEnv(&raw_tap) }).ok_or(ExecError::NodeFault("malloc tap env"))?;
    let mut error: c_int = 0;
    // SAFETY: valid C objects and buffers; `ihr` and `amr` may be null (as in Elements).
    let ok = unsafe {
        exec_simplicity(
            &mut error,
            std::ptr::null_mut(),
            tx.0.as_ptr(),
            ix as c_uint_fast32_t,
            tap.as_ptr(),
            genesis_hash.as_ptr(),
            0,
            budget,
            std::ptr::null(),
            program.as_ptr(),
            program.len(),
            witness.as_ptr(),
            witness.len(),
        )
    };
    // SAFETY: allocated by `elements_mallocTapEnv`, freed exactly once.
    unsafe { free_tap_env(tap.as_ptr()) };
    if !ok {
        return Err(ExecError::NodeFault("execSimplicity internal error"));
    }
    match error {
        NO_ERROR => Ok(()),
        ERR_MALLOC | ERR_NOT_YET_IMPLEMENTED => Err(ExecError::NodeFault(error_name(error))),
        code => Err(ExecError::Rejected { code, name: error_name(code) }),
    }
}
