//! M4: confidential values and assets, issuance, reissuance, and the attacks the amount port
//! must reject. Transactions are blinded with rust-elements (`Transaction::blind`), the same
//! code wallets use.

mod common;

use common::*;
use elements::confidential::{Asset, AssetBlindingFactor, Nonce, Value, ValueBlindingFactor};
use elements::secp256k1_zkp::{PublicKey, RangeProof as ZkRangeProof, SecretKey, SECP256K1};
use elements::{
    AssetBlindingNonce, AssetEntropy, AssetId, AssetIssuance, OutPoint, RangeProof, Transaction, TxOut, TxOutSecrets,
};
use rand::rngs::StdRng;
use rand::SeedableRng;
use wharfedale_ledger::{check_tx, LedgerView, MemoryLedger, ProofLoc, TxError, UtxoEntry};

fn blinding(seed: u8) -> (SecretKey, PublicKey) {
    let sk = SecretKey::from_slice(&[seed; 32]).unwrap();
    (sk, PublicKey::from_secret_key(SECP256K1, &sk))
}

fn explicit_secrets(asset: AssetId, value: u64) -> TxOutSecrets {
    TxOutSecrets::new(asset, AssetBlindingFactor::zero(), value, ValueBlindingFactor::zero())
}

/// An explicit output marked for blinding to `blinder`.
fn to_blind(key: &Key, value: u64, asset: AssetId, blinder: PublicKey) -> TxOut {
    let mut out = explicit(&key.spk, value, asset);
    out.nonce = Nonce::Confidential(blinder);
    out
}

fn run(view: &MemoryLedger, tx: &Transaction) -> Result<wharfedale_ledger::FeeMap, TxError> {
    check_tx(view, tx, &ctx())
}

/// alice (explicit 100 000) → bob 60 000 confidential + alice change 39 000 confidential + fee.
fn confidential_payment() -> (Key, Key, MemoryLedger, Transaction) {
    let (alice, bob) = (Key::new(1), Key::new(2));
    let ledger = ledger_with(&[(1, &alice)]);
    let mut tx = unsigned(
        &[outpoint(1)],
        vec![to_blind(&bob, 60_000, asset(), blinding(20).1), to_blind(&alice, 39_000, asset(), blinding(10).1), fee(1_000)],
    );
    tx.blind(&mut StdRng::seed_from_u64(1), SECP256K1, &[explicit_secrets(asset(), 100_000)], false).unwrap();
    let tx = sign_all(&ledger, tx, &alice);
    (alice, bob, ledger, tx)
}

#[test]
fn confidential_payment_is_valid_and_unblinds() {
    let (_, _, ledger, tx) = confidential_payment();
    assert!(tx.output[0].value.is_confidential() && tx.output[0].asset.is_confidential());
    assert_eq!(run(&ledger, &tx).unwrap().get(&asset()), Some(&1_000));
    let secrets = tx.output[0].unblind(SECP256K1, blinding(20).0).unwrap();
    assert_eq!((secrets.asset, secrets.value), (asset(), 60_000));
    assert!(tx.output[0].unblind(SECP256K1, blinding(10).0).is_err(), "wrong blinding key");
}

#[test]
fn spending_a_confidential_output() {
    let (_, bob, mut ledger, tx) = confidential_payment();
    let coin = OutPoint::new(tx.txid(), 0);
    let secrets = tx.output[0].unblind(SECP256K1, blinding(20).0).unwrap();
    ledger.insert(coin, UtxoEntry::new(tx.output[0].clone(), 1, false));

    let carol = Key::new(3);
    let mut spend = unsigned(
        &[coin],
        vec![to_blind(&carol, 50_000, asset(), blinding(30).1), to_blind(&bob, 9_000, asset(), blinding(20).1), fee(1_000)],
    );
    spend.blind(&mut StdRng::seed_from_u64(2), SECP256K1, &[secrets], false).unwrap();
    let spend = sign_all(&ledger, spend, &bob);
    assert!(run(&ledger, &spend).is_ok());
    assert_eq!(spend.output[0].unblind(SECP256K1, blinding(30).0).unwrap().value, 50_000);
}

#[test]
fn tampered_proofs_and_amounts_are_rejected() {
    let (alice, _, ledger, tx) = confidential_payment();
    let resign = |t: Transaction| sign_all(&ledger, t, &alice);

    // Inflation: a bigger explicit fee breaks the Pedersen balance.
    let mut t = tx.clone();
    t.output[2] = fee(1_001);
    assert_eq!(run(&ledger, &resign(t)), Err(TxError::BalanceMismatch));

    // Swapped value commitments (both confidential, same total): rangeproofs no longer match.
    let mut t = tx.clone();
    let (a, b) = (t.output[0].value, t.output[1].value);
    t.output[0].value = b;
    t.output[1].value = a;
    assert_eq!(run(&ledger, &resign(t)), Err(TxError::BadRangeproof(ProofLoc::Output(0))));

    // A rangeproof moved to another output is bound to the other script.
    let mut t = tx.clone();
    t.output[0].witness.rangeproof = t.output[1].witness.rangeproof.clone();
    assert_eq!(run(&ledger, &resign(t)), Err(TxError::BadRangeproof(ProofLoc::Output(0))));

    // Missing proofs.
    let mut t = tx.clone();
    t.output[1].witness.rangeproof = RangeProof::EMPTY;
    assert_eq!(run(&ledger, &resign(t)), Err(TxError::MissingRangeproof(ProofLoc::Output(1))));
    let mut t = tx.clone();
    t.output[0].witness.surjection_proof = elements::SurjectionProof::EMPTY;
    assert_eq!(run(&ledger, &resign(t)), Err(TxError::MissingSurjectionProof(0)));

    // A surjection proof from another output proves a different generator.
    let mut t = tx.clone();
    t.output[0].witness.surjection_proof = t.output[1].witness.surjection_proof.clone();
    assert_eq!(run(&ledger, &resign(t)), Err(TxError::BadSurjectionProof(0)));
}

#[test]
fn rangeproof_allowing_zero_on_spendable_output_is_rejected() {
    let (alice, bob, ledger, tx) = confidential_payment();
    let secrets = tx.output[0].unblind(SECP256K1, blinding(20).0).unwrap();
    let mut t = tx.clone();
    let (Value::Confidential(commitment), Asset::Confidential(gen)) = (t.output[0].value, t.output[0].asset) else {
        unreachable!()
    };
    // Same commitment, re-proven with min_value = 0.
    let proof = ZkRangeProof::new(
        SECP256K1,
        0,
        commitment,
        secrets.value,
        secrets.value_bf.into_inner(),
        &[],
        bob.spk.as_bytes(),
        SecretKey::from_slice(&[7; 32]).unwrap(),
        0,
        52,
        gen,
    )
    .unwrap();
    t.output[0].witness.rangeproof = RangeProof::from_slice(&proof.serialize()).unwrap();
    let t = sign_all(&ledger, t, &alice);
    assert_eq!(run(&ledger, &t), Err(TxError::RangeproofMinValueZero(ProofLoc::Output(0))));
}

#[test]
fn full_64_bit_rangeproof_does_not_panic() {
    // max_value = u64::MAX: secp256k1-zkp computes `max_value + 1` (overflow checks are off
    // for that crate in every profile; see the workspace Cargo.toml).
    let (alice, bob, ledger, tx) = confidential_payment();
    let secrets = tx.output[0].unblind(SECP256K1, blinding(20).0).unwrap();
    let mut t = tx.clone();
    let (Value::Confidential(commitment), Asset::Confidential(gen)) = (t.output[0].value, t.output[0].asset) else {
        unreachable!()
    };
    let proof = ZkRangeProof::new(
        SECP256K1,
        0,
        commitment,
        secrets.value,
        secrets.value_bf.into_inner(),
        &[],
        bob.spk.as_bytes(),
        SecretKey::from_slice(&[7; 32]).unwrap(),
        0,
        64,
        gen,
    )
    .unwrap();
    t.output[0].witness.rangeproof = RangeProof::from_slice(&proof.serialize()).unwrap();
    let t = sign_all(&ledger, t, &alice);
    assert_eq!(run(&ledger, &t), Err(TxError::RangeproofMinValueZero(ProofLoc::Output(0))));
}

// ---- issuance ----

/// alice issues 1 000 units (explicit) and one reissuance token (confidential output),
/// paying the fee from the same input.
fn issuance() -> (Key, MemoryLedger, Transaction, AssetId, AssetId) {
    let alice = Key::new(1);
    let ledger = ledger_with(&[(1, &alice)]);
    let mut tx = unsigned(&[outpoint(1)], vec![]);
    tx.input[0].asset_issuance = AssetIssuance {
        asset_blinding_nonce: AssetBlindingNonce::NEW_ISSUANCE,
        asset_entropy: AssetEntropy::NEW_ISSUANCE, // contract hash = 0
        amount: Value::Explicit(1_000),
        inflation_keys: Value::Explicit(1),
    };
    let (new_asset, token) = tx.input[0].issuance_ids();
    tx.output = vec![
        explicit(&alice.spk, 1_000, new_asset),
        to_blind(&alice, 1, token, blinding(10).1),
        to_blind(&alice, 99_000, asset(), blinding(10).1),
        fee(1_000),
    ];
    // Surjection domain in consensus order: input, issued asset, token.
    let domain = [explicit_secrets(asset(), 100_000), explicit_secrets(new_asset, 1_000), explicit_secrets(token, 1)];
    tx.blind(&mut StdRng::seed_from_u64(3), SECP256K1, &domain, false).unwrap();
    let tx = sign_all(&ledger, tx, &alice);
    (alice, ledger, tx, new_asset, token)
}

#[test]
fn new_issuance_is_valid() {
    let (_, ledger, tx, new_asset, token) = issuance();
    assert_eq!(new_asset, AssetId::new_issuance(outpoint(1), elements::ContractHash::from_byte_array([0; 32])));
    assert!(run(&ledger, &tx).is_ok());
    assert_eq!(tx.output[1].unblind(SECP256K1, blinding(10).0).unwrap().asset, token);
}

#[test]
fn issuance_rules() {
    let (alice, ledger, tx, _, _) = issuance();
    let resign = |t: Transaction| sign_all(&ledger, t, &alice);

    // Issuing more than the outputs carry (or vice versa) breaks the balance.
    let mut t = tx.clone();
    t.input[0].asset_issuance.amount = Value::Explicit(1_001);
    assert_eq!(run(&ledger, &resign(t)), Err(TxError::BalanceMismatch));

    // Explicit zero issuance.
    let mut t = tx.clone();
    t.input[0].asset_issuance.amount = Value::Explicit(0);
    assert_eq!(run(&ledger, &resign(t)), Err(TxError::IssuanceAmountInvalid(ProofLoc::IssuanceAmount(0))));

    // An issuance rangeproof next to an explicit amount.
    let mut t = tx.clone();
    t.input[0].witness.amount_rangeproof = t.output[1].witness.rangeproof.clone();
    assert_eq!(run(&ledger, &resign(t)), Err(TxError::UnexpectedIssuanceProof(0)));
}

/// Reissue 500 more by spending the token output of `issuance()`.
fn reissuance() -> (Key, MemoryLedger, Transaction, AssetId) {
    let (alice, mut ledger, iss, new_asset, token) = issuance();
    let token_coin = OutPoint::new(iss.txid(), 1);
    let fee_coin = OutPoint::new(iss.txid(), 2);
    ledger.insert(token_coin, UtxoEntry::new(iss.output[1].clone(), 1, false));
    ledger.insert(fee_coin, UtxoEntry::new(iss.output[2].clone(), 1, false));
    let token_secrets = iss.output[1].unblind(SECP256K1, blinding(10).0).unwrap();
    let fee_secrets = iss.output[2].unblind(SECP256K1, blinding(10).0).unwrap();

    let mut tx = unsigned(&[token_coin, fee_coin], vec![]);
    tx.input[0].asset_issuance = AssetIssuance {
        asset_blinding_nonce: AssetBlindingNonce::from_blinding_factor(token_secrets.asset_bf),
        asset_entropy: AssetId::generate_asset_entropy(outpoint(1), elements::ContractHash::from_byte_array([0; 32])),
        amount: Value::Explicit(500),
        inflation_keys: Value::Null,
    };
    assert_eq!(tx.input[0].issuance_ids().0, new_asset);
    tx.output = vec![
        explicit(&alice.spk, 500, new_asset),
        to_blind(&alice, 1, token, blinding(10).1),
        to_blind(&alice, 98_000, asset(), blinding(10).1),
        fee(1_000),
    ];
    let domain = [token_secrets, explicit_secrets(new_asset, 500), fee_secrets];
    tx.blind(&mut StdRng::seed_from_u64(4), SECP256K1, &domain, false).unwrap();
    let tx = sign_all(&ledger, tx, &alice);
    (alice, ledger, tx, new_asset)
}

#[test]
fn reissuance_with_the_token_is_valid() {
    let (_, ledger, tx, _) = reissuance();
    assert!(run(&ledger, &tx).is_ok());
}

#[test]
fn reissuance_rules() {
    let (alice, ledger, tx, _) = reissuance();
    let resign = |t: Transaction| sign_all(&ledger, t, &alice);

    // A nonce that does not open the spent token commitment.
    let mut t = tx.clone();
    t.input[0].asset_issuance.asset_blinding_nonce = AssetBlindingNonce::from_byte_array([5; 32]);
    assert_eq!(run(&ledger, &resign(t)), Err(TxError::ReissuanceTokenMismatch(0)));

    // Reissuance from an input that is not the token (the fee coin).
    let mut t = tx.clone();
    t.input.swap(0, 1);
    let iss = t.input[1].asset_issuance;
    t.input[1].asset_issuance = AssetIssuance::null();
    t.input[0].asset_issuance = iss;
    assert_eq!(run(&ledger, &resign(t)), Err(TxError::ReissuanceTokenMismatch(0)));

    // Reissuance can't mint tokens.
    let mut t = tx.clone();
    t.input[0].asset_issuance.inflation_keys = Value::Explicit(1);
    assert_eq!(run(&ledger, &resign(t)), Err(TxError::ReissuanceCreatesTokens(0)));
}

#[test]
fn ledger_view_is_used_for_confidential_prevouts() {
    // The UTXO set stores outputs without proofs; validation needs only the commitments.
    let (_, _, _, tx) = confidential_payment();
    let entry = UtxoEntry::new(tx.output[0].clone(), 1, false);
    assert!(entry.txout.witness.is_empty());
    let mut l = MemoryLedger::new();
    l.insert(OutPoint::new(tx.txid(), 0), entry.clone());
    assert_eq!(l.utxo(&OutPoint::new(tx.txid(), 0)), Some(entry));
}
