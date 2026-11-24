//! FFI tests: SimplicityHL `p2pk` compiled, satisfied against rust-simplicity's environment
//! (independent transaction encoding), executed through `exec`.

use std::sync::Arc;

use elements::secp256k1_zkp::{Keypair, Message, SECP256K1};
use elements::{confidential, AssetId, BlockHash, OutPoint, Script, Sequence, Transaction, TxIn, TxOut, Txid};
use simplicity::jet::elements::{ElementsEnv, ElementsUtxo};
use simplicity::Cmr;
use simplicityhl::{ast::ElementsJetHinter, CompiledProgram, TemplateAst, UnresolvedValues};
use wharfedale_ledger_simplicity_sys::{exec, taproot, CTx, ExecError};

const P2PK: &str = "fn main() {
    jet::bip_0340_verify((param::OWNER, jet::sig_all_hash()), witness::SIG)
}";
const GENESIS: [u8; 32] = [7; 32];

fn compile(owner: &Keypair) -> CompiledProgram {
    let tmpl = TemplateAst::new(P2PK, Box::new(ElementsJetHinter::new())).expect("parses");
    let args = format!(r#"{{"OWNER": "0x{}"}}"#, owner.x_only_public_key().0);
    let args = serde_json::from_str::<UnresolvedValues>(&args).unwrap().resolve(tmpl.parameters()).unwrap();
    tmpl.instantiate(args, false).expect("compiles")
}

fn cmr(p: &CompiledProgram) -> [u8; 32] {
    p.commit().cmr().to_byte_array()
}

struct Spend {
    tx: Transaction,
    prev: TxOut,
    program: Vec<u8>,
    witness: Vec<u8>,
    cmr: [u8; 32],
}

fn explicit(spk: Script, value: u64) -> TxOut {
    TxOut {
        asset: confidential::Asset::Explicit(AssetId::LIQUID_BTC),
        value: confidential::Value::Explicit(value),
        nonce: confidential::Nonce::Null,
        script_pubkey: spk,
        witness: Default::default(),
    }
}

/// A transaction spending a p2pk contract output owned by `owner`, signed by `signer`.
/// (SimplicityHL prunes by running the program, so `signer` must be the owner.)
fn spend(owner: &Keypair, signer: &Keypair) -> Spend {
    let program = compile(owner);
    let cmr = cmr(&program);
    let prev = explicit(taproot::script_pubkey(&cmr), 10_000);
    let tx = Transaction {
        version: 2,
        lock_time: elements::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([3; 32]), 1),
            is_pegin: false,
            script_sig: Script::new(),
            sequence: Sequence::MAX,
            asset_issuance: Default::default(),
            witness: Default::default(),
        }],
        output: vec![explicit(Script::from(vec![0x51, 0x20, 0x00]), 9_000), explicit(Script::new(), 1_000)],
    };
    let env = ElementsEnv::new(
        Arc::new(tx.clone()),
        vec![ElementsUtxo::from(prev.clone())],
        0,
        Cmr::from_byte_array(cmr),
        taproot::control_block(&cmr),
        None,
        BlockHash::from_byte_array(GENESIS),
    );
    let sighash = env.c_tx_env().sighash_all();
    let sig = SECP256K1.sign_schnorr_no_aux_rand(&Message::from_digest(sighash.to_byte_array()), signer);
    let wit = format!(r#"{{"SIG": "0x{}"}}"#, hex(sig.as_ref()));
    let wit = serde_json::from_str::<UnresolvedValues>(&wit).unwrap().resolve(program.witness_types()).unwrap();
    let satisfied = program.satisfy_with_env(wit, Some(&env)).expect("satisfies");
    let (program, witness) = satisfied.redeem().to_vec_with_witness();
    Spend { tx, prev, program, witness, cmr }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn run(s: &Spend, cmr: &[u8; 32], budget: i64) -> Result<(), ExecError> {
    let ctx = CTx::new(&s.tx, std::slice::from_ref(&s.prev))?;
    let cb = taproot::control_block(&s.cmr).serialize();
    exec(&ctx, 0, &cb, cmr, &GENESIS, &s.program, &s.witness, budget)
}

fn key(b: u8) -> Keypair {
    Keypair::from_seckey_slice(SECP256K1, &[b; 32]).unwrap()
}

#[test]
fn p2pk_owner_signature_executes() {
    let s = spend(&key(1), &key(1));
    run(&s, &s.cmr, 10_000).expect("valid spend");
}

#[test]
fn p2pk_corrupted_signature_fails_jet() {
    let mut s = spend(&key(1), &key(1));
    let mid = s.witness.len() / 2; // inside the 64-byte signature
    s.witness[mid] ^= 1;
    let err = run(&s, &s.cmr, 10_000).unwrap_err();
    assert!(matches!(err, ExecError::Rejected { name: "EXEC_JET", .. }), "{err:?}");
}

#[test]
fn modified_transaction_fails_signature() {
    let mut s = spend(&key(1), &key(1));
    s.tx.output[0].value = confidential::Value::Explicit(8_000);
    s.tx.output[1].value = confidential::Value::Explicit(2_000);
    let err = run(&s, &s.cmr, 10_000).unwrap_err();
    assert!(matches!(err, ExecError::Rejected { name: "EXEC_JET", .. }), "{err:?}");
}

#[test]
fn wrong_cmr_rejected() {
    let s = spend(&key(1), &key(1));
    let other = cmr(&compile(&key(3)));
    let err = run(&s, &other, 10_000).unwrap_err();
    assert!(matches!(err, ExecError::Rejected { name: "CMR", .. }), "{err:?}");
}

#[test]
fn budget_enforced() {
    let s = spend(&key(1), &key(1));
    let err = run(&s, &s.cmr, 1).unwrap_err();
    assert!(matches!(err, ExecError::Rejected { name: "EXEC_BUDGET", .. }), "{err:?}");
}

#[test]
fn garbage_program_rejected() {
    let mut s = spend(&key(1), &key(1));
    s.program = vec![0xff; 8];
    assert!(matches!(run(&s, &s.cmr, 10_000), Err(ExecError::Rejected { .. })));
}

#[test]
fn contract_output_is_simplicity_only() {
    let c = [9u8; 32];
    let spk = taproot::script_pubkey(&c);
    assert!(spk.is_v1_p2tr());
    let cb = taproot::control_block(&c);
    assert_eq!(cb.leaf_version.as_u8(), taproot::LEAF_VERSION);
    assert_eq!(cb.internal_key, taproot::nums_key());
    let q = elements::schnorr::TweakedPublicKey::new(
        elements::secp256k1_zkp::XOnlyPublicKey::from_slice(&spk.as_bytes()[2..]).unwrap(),
    );
    assert!(cb.verify_taproot_commitment(SECP256K1, &q, &Script::from(c.to_vec())));
}
