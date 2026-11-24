//! Simplicity spends (leaf `0xbe`) through `check_tx`, with SimplicityHL contracts.

mod common;

use std::sync::Arc;

use common::*;
use elements::secp256k1_zkp::{Keypair, Message, SECP256K1};
use elements::{LockTime, Script, Sequence, Transaction, TxOut, Witness};
use simplicity::jet::elements::{ElementsEnv, ElementsUtxo};
use simplicity::Cmr;
use simplicityhl::{ast::ElementsJetHinter, CompiledProgram, TemplateAst, UnresolvedValues};
use wharfedale_ledger::{check_tx, MemoryLedger, TxError, UtxoEntry, VerifyCache};
use wharfedale_ledger_simplicity_sys::taproot;

const P2PK: &str = "fn main() {
    jet::bip_0340_verify((param::OWNER, jet::sig_all_hash()), witness::SIG)
}";

/// Anyone can spend once the transaction's lock time is at least height 5.
const AFTER_5: &str = "fn main() {
    let h: Height = 5;
    jet::check_lock_height(h);
}";

struct Contract {
    program: CompiledProgram,
    cmr: [u8; 32],
    spk: Script,
}

fn contract(src: &str, args: &str) -> Contract {
    let tmpl = TemplateAst::new(src, Box::new(ElementsJetHinter::new())).expect("parses");
    let args = serde_json::from_str::<UnresolvedValues>(args).unwrap().resolve(tmpl.parameters()).unwrap();
    let program = tmpl.instantiate(args, false).expect("compiles");
    let cmr = program.commit().cmr().to_byte_array();
    Contract { program, cmr, spk: taproot::script_pubkey(&cmr) }
}

fn p2pk(owner: &Keypair) -> Contract {
    contract(P2PK, &format!(r#"{{"OWNER": "0x{}"}}"#, owner.x_only_public_key().0))
}

fn env(c: &Contract, tx: &Transaction, ix: u32, prevouts: &[TxOut]) -> ElementsEnv<Arc<Transaction>> {
    ElementsEnv::new(
        Arc::new(tx.clone()),
        prevouts.iter().cloned().map(ElementsUtxo::from).collect(),
        ix,
        Cmr::from_byte_array(c.cmr),
        taproot::control_block(&c.cmr),
        None,
        genesis(),
    )
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Witness stack for input `ix` satisfying `c` with witness values `wit` (JSON).
fn satisfy(c: &Contract, tx: &Transaction, ix: u32, prevouts: &[TxOut], wit: &str) -> Witness {
    let env = env(c, tx, ix, prevouts);
    let wit = serde_json::from_str::<UnresolvedValues>(wit).unwrap().resolve(c.program.witness_types()).unwrap();
    let (program, witness) = c.program.satisfy_with_env(wit, Some(&env)).expect("satisfies").redeem().to_vec_with_witness();
    taproot::spend_witness(&witness, &program, &c.cmr)
}

fn sig_all(c: &Contract, tx: &Transaction, ix: u32, prevouts: &[TxOut], key: &Keypair) -> String {
    let hash = env(c, tx, ix, prevouts).c_tx_env().sighash_all();
    let sig = SECP256K1.sign_schnorr_no_aux_rand(&Message::from_digest(hash.to_byte_array()), key);
    format!(r#"{{"SIG": "0x{}"}}"#, hex(sig.as_ref()))
}

fn keypair(seed: u8) -> Keypair {
    Keypair::from_seckey_slice(SECP256K1, &[seed; 32]).unwrap()
}

fn locked(spk: &Script) -> (MemoryLedger, Vec<TxOut>) {
    let mut ledger = MemoryLedger::new();
    let prev = explicit(spk, 100_000, asset());
    ledger.insert(outpoint(1), UtxoEntry::new(prev.clone(), 1, false));
    (ledger, vec![prev])
}

fn run(ledger: &MemoryLedger, tx: &Transaction) -> Result<wharfedale_ledger::FeeMap, TxError> {
    check_tx(ledger, tx, &ctx(), &VerifyCache::new())
}

/// A p2pk contract output spent to `Key::new(2)`, signed by the owner.
fn p2pk_spend() -> (MemoryLedger, Contract, Transaction) {
    let owner = keypair(1);
    let c = p2pk(&owner);
    let (ledger, prevouts) = locked(&c.spk);
    let mut tx = unsigned(&[outpoint(1)], vec![explicit(&Key::new(2).spk, 99_000, asset()), fee(1_000)]);
    let wit = sig_all(&c, &tx, 0, &prevouts, &owner);
    tx.input[0].witness.script_witness = satisfy(&c, &tx, 0, &prevouts, &wit);
    (ledger, c, tx)
}

#[test]
fn p2pk_contract_spend_is_valid() {
    let (ledger, _, tx) = p2pk_spend();
    let cache = VerifyCache::new();
    let fees = check_tx(&ledger, &tx, &ctx(), &cache).unwrap();
    assert_eq!(fees.get(&asset()), Some(&1_000));
    assert!(cache.contains(tx.wtxid()));
}

#[test]
fn p2pk_contract_spend_with_annex_is_valid() {
    let owner = keypair(1);
    let c = p2pk(&owner);
    let (ledger, prevouts) = locked(&c.spk);
    let mut tx = unsigned(&[outpoint(1)], vec![explicit(&Key::new(2).spk, 99_000, asset()), fee(1_000)]);
    // The annex is part of the signed data: attach it before computing the sighash.
    tx.input[0].witness.script_witness = Witness::from_slice(&[vec![0x50, 0xaa]]);
    let wit = sig_all(&c, &tx, 0, &prevouts, &owner);
    let mut stack: Vec<Vec<u8>> = satisfy(&c, &tx, 0, &prevouts, &wit).iter().map(<[u8]>::to_vec).collect();
    stack.push(vec![0x50, 0xaa]);
    tx.input[0].witness.script_witness = Witness::from_slice(&stack);
    run(&ledger, &tx).unwrap();
}

#[test]
fn changed_outputs_break_the_signature() {
    let (ledger, _, mut tx) = p2pk_spend();
    tx.output[0].value = elements::confidential::Value::Explicit(98_000);
    tx.output[1].value = elements::confidential::Value::Explicit(2_000);
    assert_eq!(run(&ledger, &tx), Err(TxError::SimplicityRejected(0, "EXEC_JET")));
}

#[test]
fn program_must_match_the_committed_cmr() {
    // Spend the output locked to `c` with a different program (and its own CMR/control block).
    let (ledger, _, mut tx) = p2pk_spend();
    let other = p2pk(&keypair(3));
    let stack: Vec<Vec<u8>> = tx.input[0].witness.script_witness.iter().map(<[u8]>::to_vec).collect();
    tx.input[0].witness.script_witness = taproot::spend_witness(&stack[0], &stack[1], &other.cmr);
    assert_eq!(run(&ledger, &tx), Err(TxError::TaprootCommitment(0)));
}

#[test]
fn cmr_element_must_match_the_program() {
    // Correct CMR and control block, but the program bytes encode another program.
    let (ledger, _, mut tx) = p2pk_spend();
    let (_, _, other) = {
        let owner = keypair(3);
        let c = p2pk(&owner);
        let (l, prevouts) = locked(&c.spk);
        let mut t = tx.clone();
        let wit = sig_all(&c, &t, 0, &prevouts, &owner);
        t.input[0].witness.script_witness = satisfy(&c, &t, 0, &prevouts, &wit);
        (l, c, t)
    };
    let mut stack: Vec<Vec<u8>> = tx.input[0].witness.script_witness.iter().map(<[u8]>::to_vec).collect();
    let foreign: Vec<Vec<u8>> = other.input[0].witness.script_witness.iter().map(<[u8]>::to_vec).collect();
    stack[0] = foreign[0].clone();
    stack[1] = foreign[1].clone();
    tx.input[0].witness.script_witness = Witness::from_slice(&stack);
    assert_eq!(run(&ledger, &tx), Err(TxError::SimplicityRejected(0, "CMR")));
}

#[test]
fn witness_shape_and_control_block_are_checked() {
    let (ledger, _, tx) = p2pk_spend();
    let stack: Vec<Vec<u8>> = tx.input[0].witness.script_witness.iter().map(<[u8]>::to_vec).collect();

    let mut three = tx.clone();
    three.input[0].witness.script_witness = Witness::from_slice(&stack[1..]);
    assert_eq!(run(&ledger, &three), Err(TxError::WitnessShape(0)));

    let mut short_cmr = tx.clone();
    let mut s = stack.clone();
    s[2].pop();
    short_cmr.input[0].witness.script_witness = Witness::from_slice(&s);
    assert_eq!(run(&ledger, &short_cmr), Err(TxError::WitnessShape(0)));

    let mut bad_cb = tx.clone();
    let mut s = stack.clone();
    s[3].push(0);
    bad_cb.input[0].witness.script_witness = Witness::from_slice(&s);
    assert_eq!(run(&ledger, &bad_cb), Err(TxError::ControlBlock(0)));
}

#[test]
fn simplicity_and_key_path_inputs_mix() {
    let owner = keypair(1);
    let c = p2pk(&owner);
    let a = Key::new(4);
    let mut ledger = ledger_with(&[(2, &a)]);
    let prev = explicit(&c.spk, 100_000, asset());
    ledger.insert(outpoint(1), UtxoEntry::new(prev.clone(), 1, false));
    let prevouts = vec![prev, explicit(&a.spk, 100_000, asset())];
    let mut tx = unsigned(&[outpoint(1), outpoint(2)], vec![explicit(&Key::new(2).spk, 199_000, asset()), fee(1_000)]);
    let sig = a.sign(&tx, 1, &prevouts, elements::SchnorrSighashType::Default);
    tx.input[1].witness.script_witness = Witness::from_slice(&[sig]);
    let wit = sig_all(&c, &tx, 0, &prevouts, &owner);
    tx.input[0].witness.script_witness = satisfy(&c, &tx, 0, &prevouts, &wit);
    assert_eq!(run(&ledger, &tx).unwrap().get(&asset()), Some(&1_000));
}

#[test]
fn covenant_reads_the_transaction_lock_time() {
    let c = contract(AFTER_5, "{}");
    let (ledger, prevouts) = locked(&c.spk);
    let mut tx = unsigned(&[outpoint(1)], vec![explicit(&Key::new(2).spk, 99_000, asset()), fee(1_000)]);
    tx.input[0].sequence = Sequence::ENABLE_LOCKTIME_NO_RBF;
    tx.lock_time = LockTime::from_height(5).unwrap();
    tx.input[0].witness.script_witness = satisfy(&c, &tx, 0, &prevouts, "{}");
    run(&ledger, &tx).expect("lock time 5 satisfies the contract");

    // Same program and witness, earlier lock time (still final at the chain height).
    tx.lock_time = LockTime::from_height(4).unwrap();
    assert_eq!(run(&ledger, &tx), Err(TxError::SimplicityRejected(0, "EXEC_JET")));
}
