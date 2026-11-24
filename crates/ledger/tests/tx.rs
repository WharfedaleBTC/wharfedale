//! `check_tx` rules for M1. One test per rule; see `docs/src/ledger/validation.md`.

mod common;

use common::*;
use elements::{LockTime, SchnorrSighashType, Script, Sequence, TxOut, Witness};
use wharfedale_ledger::consts::{SEQUENCE_LOCKTIME_DISABLE_FLAG, SEQUENCE_LOCKTIME_TYPE_FLAG};
use wharfedale_ledger::{check_tx, ChainCtx, MemoryLedger, TxError, UtxoEntry, VerifyCache};

fn run(ledger: &MemoryLedger, tx: &elements::Transaction) -> Result<wharfedale_ledger::FeeMap, TxError> {
    check_tx(ledger, tx, &ctx(), &VerifyCache::new())
}

fn simple(a: &Key, b: &Key) -> (MemoryLedger, elements::Transaction) {
    let ledger = ledger_with(&[(1, a)]);
    let tx = unsigned(&[outpoint(1)], vec![explicit(&b.spk, 99_000, asset()), fee(1_000)]);
    let tx = sign_all(&ledger, tx, a);
    (ledger, tx)
}

// --- valid paths ---

#[test]
fn key_path_spend_is_valid_and_returns_fees() {
    let (a, b) = (Key::new(1), Key::new(2));
    let (ledger, tx) = simple(&a, &b);
    let cache = VerifyCache::new();
    let fees = check_tx(&ledger, &tx, &ctx(), &cache).unwrap();
    assert_eq!(fees.get(&asset()), Some(&1_000));
    assert!(cache.contains(tx.wtxid()));
}

#[test]
fn explicit_sighash_all_byte_is_valid() {
    let (a, b) = (Key::new(1), Key::new(2));
    let ledger = ledger_with(&[(1, &a)]);
    let mut tx = unsigned(&[outpoint(1)], vec![explicit(&b.spk, 99_000, asset()), fee(1_000)]);
    let prevouts = vec![ledger_utxo(&ledger, 1)];
    let sig = a.sign(&tx, 0, &prevouts, SchnorrSighashType::All);
    assert_eq!(sig.len(), 65);
    tx.input[0].witness.script_witness = Witness::from_slice(&[sig]);
    assert!(run(&ledger, &tx).is_ok());
}

#[test]
fn zero_value_op_return_is_valid() {
    let (a, b) = (Key::new(1), Key::new(2));
    let ledger = ledger_with(&[(1, &a)]);
    let tx = unsigned(
        &[outpoint(1)],
        vec![explicit(&b.spk, 99_000, asset()), explicit(&Script::new_op_return(b"memo"), 0, asset()), fee(1_000)],
    );
    let tx = sign_all(&ledger, tx, &a);
    assert!(run(&ledger, &tx).is_ok());
}

// --- spends ---

#[test]
fn wrong_key_is_rejected() {
    let (a, b) = (Key::new(1), Key::new(2));
    let ledger = ledger_with(&[(1, &a)]);
    let tx = unsigned(&[outpoint(1)], vec![explicit(&b.spk, 99_000, asset()), fee(1_000)]);
    let tx = sign_all(&ledger, tx, &b);
    assert_eq!(run(&ledger, &tx), Err(TxError::BadSignature(0)));
}

#[test]
fn change_after_signing_is_rejected() {
    let (a, b) = (Key::new(1), Key::new(2));
    let (ledger, mut tx) = simple(&a, &b);
    tx.output[0] = explicit(&b.spk, 98_000, asset());
    tx.output[1] = fee(2_000);
    assert_eq!(run(&ledger, &tx), Err(TxError::BadSignature(0)));
}

#[test]
fn wrong_genesis_is_rejected() {
    let (a, b) = (Key::new(1), Key::new(2));
    let (ledger, tx) = simple(&a, &b);
    let other = ChainCtx { genesis_hash: elements::BlockHash::from_byte_array([1; 32]), ..ctx() };
    assert_eq!(check_tx(&ledger, &tx, &other, &VerifyCache::new()), Err(TxError::BadSignature(0)));
}

#[test]
fn sixty_five_byte_signature_with_default_type_is_rejected() {
    let (a, b) = (Key::new(1), Key::new(2));
    let (ledger, mut tx) = simple(&a, &b);
    let mut sig = tx.input[0].witness.script_witness.to_vec()[0].clone();
    sig.push(0x00);
    tx.input[0].witness.script_witness = Witness::from_slice(&[sig]);
    assert_eq!(run(&ledger, &tx), Err(TxError::SignatureEncoding(0)));
}

#[test]
fn empty_witness_is_rejected() {
    let (a, b) = (Key::new(1), Key::new(2));
    let ledger = ledger_with(&[(1, &a)]);
    let tx = unsigned(&[outpoint(1)], vec![explicit(&b.spk, 99_000, asset()), fee(1_000)]);
    assert_eq!(run(&ledger, &tx), Err(TxError::WitnessEmpty(0)));
}

#[test]
fn tapscript_leaf_is_rejected() {
    let (a, b) = (Key::new(1), Key::new(2));
    let ledger = ledger_with(&[(1, &a)]);
    let mut tx = unsigned(&[outpoint(1)], vec![explicit(&b.spk, 99_000, asset()), fee(1_000)]);
    tx.input[0].witness.script_witness = Witness::from_slice(&[vec![0x51], vec![0xc4; 33]]);
    assert_eq!(run(&ledger, &tx), Err(TxError::LeafVersionForbidden(0, 0xc4)));
}

// --- amounts ---

#[test]
fn explicit_output_total_is_capped() {
    // Two outputs each within MAX_MONEY, total above it (Elements `bad-txns-txouttotal-toolarge`).
    let (a, b) = (Key::new(1), Key::new(2));
    let ledger = ledger_with(&[(1, &a)]);
    let half = wharfedale_ledger::consts::MAX_MONEY / 2 + 1;
    let tx = unsigned(&[outpoint(1)], vec![explicit(&b.spk, half, asset()), explicit(&b.spk, half, other_asset()), fee(1_000)]);
    assert_eq!(run(&ledger, &sign_all(&ledger, tx, &a)), Err(TxError::OutputTotalOutOfRange));
}

#[test]
fn failed_transactions_are_not_cached() {
    // Amounts are valid, the signature is not: nothing may be cached.
    let (a, b) = (Key::new(1), Key::new(2));
    let (ledger, mut tx) = simple(&a, &b);
    tx.input[0].witness.script_witness = Witness::from_slice(&[vec![1u8; 64]]);
    let cache = VerifyCache::new();
    assert_eq!(check_tx(&ledger, &tx, &ctx(), &cache), Err(TxError::BadSignature(0)));
    assert!(cache.is_empty());
}

#[test]
fn verify_cache_is_bounded() {
    let (a, b) = (Key::new(1), Key::new(2));
    let cache = VerifyCache::with_capacity(2);
    for n in 1..=3u8 {
        let ledger = ledger_with(&[(n, &a)]);
        let tx = sign_all(&ledger, unsigned(&[outpoint(n)], vec![explicit(&b.spk, 99_000, asset()), fee(1_000)]), &a);
        check_tx(&ledger, &tx, &ctx(), &cache).unwrap();
    }
    assert_eq!(cache.len(), 2);
}

#[test]
fn outputs_exceeding_inputs_are_rejected() {
    let (a, b) = (Key::new(1), Key::new(2));
    let ledger = ledger_with(&[(1, &a)]);
    let tx = unsigned(&[outpoint(1)], vec![explicit(&b.spk, 99_001, asset()), fee(1_000)]);
    let tx = sign_all(&ledger, tx, &a);
    assert_eq!(run(&ledger, &tx), Err(TxError::BalanceMismatch));
}

#[test]
fn asset_conversion_is_rejected() {
    let (a, b) = (Key::new(1), Key::new(2));
    let ledger = ledger_with(&[(1, &a)]);
    let tx = unsigned(&[outpoint(1)], vec![explicit(&b.spk, 99_000, other_asset()), fee(1_000)]);
    let tx = sign_all(&ledger, tx, &a);
    assert!(matches!(run(&ledger, &tx), Err(TxError::BalanceMismatch)));
}

// --- stateless ---

#[test]
fn stateless_rules() {
    let (a, b) = (Key::new(1), Key::new(2));
    let (ledger, base) = simple(&a, &b);

    let mut tx = base.clone();
    tx.version = 1;
    assert_eq!(run(&ledger, &tx), Err(TxError::Version(1)));

    let mut tx = base.clone();
    tx.input[0].is_pegin = true;
    assert_eq!(run(&ledger, &tx), Err(TxError::Pegin(0)));

    let mut tx = base.clone();
    tx.input[0].script_sig = Script::from(vec![0x51]);
    assert_eq!(run(&ledger, &tx), Err(TxError::ScriptSigNotEmpty(0)));

    let mut tx = base.clone();
    tx.input.push(tx.input[0].clone());
    assert_eq!(run(&ledger, &tx), Err(TxError::DuplicateInput(1)));

    let mut tx = base.clone();
    tx.input[0].previous_output = elements::OutPoint::null();
    assert_eq!(run(&ledger, &tx), Err(TxError::NullPrevout(0)));

    let mut tx = base.clone();
    tx.output.clear();
    assert_eq!(run(&ledger, &tx), Err(TxError::NoOutputs));

    let mut tx = base.clone();
    let mut p2wpkh = vec![0x00, 0x14];
    p2wpkh.extend([0u8; 20]);
    tx.output[0].script_pubkey = Script::from(p2wpkh);
    assert_eq!(run(&ledger, &tx), Err(TxError::OutputScriptForbidden(0)));

    let mut tx = base.clone();
    tx.output[0] = explicit(&b.spk, 0, asset());
    assert_eq!(run(&ledger, &tx), Err(TxError::ZeroValue(0)));

    let mut tx = base.clone();
    tx.output[1] = fee(0);
    assert_eq!(run(&ledger, &tx), Err(TxError::ZeroValue(1)));
}

#[test]
fn missing_input_is_rejected() {
    let (a, b) = (Key::new(1), Key::new(2));
    let (_, tx) = simple(&a, &b);
    assert_eq!(run(&MemoryLedger::new(), &tx), Err(TxError::MissingInput(0, outpoint(1))));
}

// --- context ---

#[test]
fn coinbase_outputs_mature_after_one_block() {
    let (a, b) = (Key::new(1), Key::new(2));
    let mut ledger = MemoryLedger::new();
    ledger.insert(outpoint(1), UtxoEntry::new(explicit(&a.spk, 100_000, asset()), HEIGHT, true));
    let tx = unsigned(&[outpoint(1)], vec![explicit(&b.spk, 99_000, asset()), fee(1_000)]);
    let tx = sign_all(&ledger, tx, &a);
    assert_eq!(run(&ledger, &tx), Err(TxError::ImmatureCoinbase(0)));
    let next = ChainCtx { height: HEIGHT + 1, ..ctx() };
    assert!(check_tx(&ledger, &tx, &next, &VerifyCache::new()).is_ok());
}

#[test]
fn absolute_lock_time() {
    let (a, b) = (Key::new(1), Key::new(2));
    let ledger = ledger_with(&[(1, &a)]);
    let build = |lock: u32, seq: u32| {
        let mut tx = unsigned(&[outpoint(1)], vec![explicit(&b.spk, 99_000, asset()), fee(1_000)]);
        tx.lock_time = LockTime::from_consensus(lock);
        tx.input[0].sequence = Sequence(seq);
        sign_all(&ledger, tx, &a)
    };
    assert!(run(&ledger, &build(HEIGHT as u32 - 1, 0xffff_fffe)).is_ok());
    assert_eq!(run(&ledger, &build(HEIGHT as u32, 0xffff_fffe)), Err(TxError::NotFinal));
    assert!(run(&ledger, &build(HEIGHT as u32 + 5, 0xffff_ffff)).is_ok(), "final sequences disable lock_time");
    assert!(run(&ledger, &build(MTP as u32 - 1, 0xffff_fffe)).is_ok());
    assert_eq!(run(&ledger, &build(MTP as u32, 0xffff_fffe)), Err(TxError::NotFinal));
}

#[test]
fn relative_lock_by_height() {
    let (a, b) = (Key::new(1), Key::new(2));
    let ledger = ledger_with(&[(1, &a)]); // coin at height 1, block at height 10
    let build = |seq: u32| {
        let mut tx = unsigned(&[outpoint(1)], vec![explicit(&b.spk, 99_000, asset()), fee(1_000)]);
        tx.input[0].sequence = Sequence(seq);
        sign_all(&ledger, tx, &a)
    };
    assert!(run(&ledger, &build(9)).is_ok());
    assert_eq!(run(&ledger, &build(10)), Err(TxError::SequenceLocked));
    assert!(run(&ledger, &build(SEQUENCE_LOCKTIME_DISABLE_FLAG | 10)).is_ok());
}

#[test]
fn relative_lock_by_time() {
    let (a, b) = (Key::new(1), Key::new(2));
    let mut ledger = ledger_with(&[(1, &a)]);
    ledger.set_median_time_past(0, MTP - 600); // MTP of the block before the coin's block
    let build = |units: u32| {
        let mut tx = unsigned(&[outpoint(1)], vec![explicit(&b.spk, 99_000, asset()), fee(1_000)]);
        tx.input[0].sequence = Sequence(SEQUENCE_LOCKTIME_TYPE_FLAG | units);
        sign_all(&ledger, tx, &a)
    };
    assert!(run(&ledger, &build(1)).is_ok()); // MTP - 600 + 512 - 1 < MTP
    assert_eq!(run(&ledger, &build(2)), Err(TxError::SequenceLocked)); // MTP - 600 + 1024 - 1 >= MTP
}

fn ledger_utxo(ledger: &MemoryLedger, n: u8) -> TxOut {
    use wharfedale_ledger::LedgerView;
    ledger.utxo(&outpoint(n)).unwrap().txout
}
