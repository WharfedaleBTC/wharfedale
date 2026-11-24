//! Mempool v1 policy: admission rejects, ordering, block eviction, gossip wire format.

use std::time::{Duration, Instant};

use elements::confidential::{Asset, Nonce, Value};
use elements::schnorr::TapTweak;
use elements::secp256k1_zkp::{Keypair, Message, SECP256K1};
use elements::sighash::{Prevouts, SighashCache};
use elements::{
    AssetId, BlockHash, LockTime, OutPoint, SchnorrSighashType, Script, Sequence, Transaction, TxIn, TxOut, TxOutWitness,
    Txid, Witness,
};
use wharfedale_ledger::{ChainCtx, LedgerView, MemoryLedger, UtxoEntry, VerifyCache};
use wharfedale_mempool::{wire, AdmitError, Mempool, Policy, Source};

fn asset() -> AssetId {
    AssetId::from_byte_array([7; 32])
}

fn ctx() -> ChainCtx {
    ChainCtx { height: 10, median_time_past: 1_800_000_000, genesis_hash: BlockHash::from_byte_array([9; 32]), coinbase_maturity: 1 }
}

fn out(spk: &Script, value: u64) -> TxOut {
    TxOut { asset: Asset::Explicit(asset()), value: Value::Explicit(value), nonce: Nonce::Null, script_pubkey: spk.clone(), witness: TxOutWitness::empty() }
}

struct Key(Keypair, Script);

impl Key {
    fn new(seed: u8) -> Self {
        let kp = Keypair::from_seckey_slice(SECP256K1, &[seed; 32]).unwrap();
        let spk = Script::new_v1_p2tr(SECP256K1, kp.x_only_public_key().0, None);
        Key(kp, spk)
    }

    /// Spends `inputs` (100 000 each in `view`) to `to`, paying `fee`.
    fn spend(&self, view: &MemoryLedger, inputs: &[OutPoint], to: &Script, fee: u64) -> Transaction {
        let total = 100_000 * inputs.len() as u64;
        let mut tx = Transaction {
            version: 2,
            lock_time: LockTime::ZERO,
            input: inputs.iter().map(|op| TxIn { previous_output: *op, sequence: Sequence::MAX, ..Default::default() }).collect(),
            output: vec![out(to, total - fee), TxOut::new_fee(fee, asset())],
        };
        let prevouts: Vec<TxOut> = inputs.iter().map(|op| view.utxo(op).unwrap().txout).collect();
        let tweaked = self.0.tap_tweak(SECP256K1, None).to_inner();
        for ix in 0..tx.input.len() {
            let h = SighashCache::new(&tx)
                .taproot_key_spend_signature_hash(ix, &Prevouts::All(&prevouts), SchnorrSighashType::Default, ctx().genesis_hash)
                .unwrap();
            let sig = SECP256K1.sign_schnorr_no_aux_rand(&Message::from_digest(h.to_byte_array()), &tweaked);
            tx.input[ix].witness.script_witness = Witness::from_slice(&[sig.serialize()]);
        }
        tx
    }
}

fn op(n: u8) -> OutPoint {
    OutPoint::new(Txid::from_byte_array([n; 32]), 0)
}

fn setup(key: &Key, n: u8) -> MemoryLedger {
    let mut l = MemoryLedger::new();
    for i in 1..=n {
        l.insert(op(i), UtxoEntry::new(out(&key.1, 100_000), 1, false));
    }
    l
}

fn admit(pool: &mut Mempool, l: &MemoryLedger, tx: Transaction) -> Result<Txid, AdmitError> {
    pool.admit(tx, Source::Local, l, &ctx(), &VerifyCache::new(), Instant::now())
}

#[test]
fn admission_rules() {
    let (a, b) = (Key::new(1), Key::new(2));
    let l = setup(&a, 3);
    let mut pool = Mempool::new(Policy::new(asset()));

    let tx = a.spend(&l, &[op(1)], &b.1, 1_000);
    let txid = admit(&mut pool, &l, tx.clone()).unwrap();
    assert_eq!(admit(&mut pool, &l, tx.clone()), Err(AdmitError::Duplicate));

    // A different transaction spending the same coin: first seen wins.
    let rival = a.spend(&l, &[op(1)], &a.1, 2_000);
    assert_eq!(admit(&mut pool, &l, rival), Err(AdmitError::Conflict(0, txid)));

    // Spending an unconfirmed (mempool) output is refused in v1.
    let mut chained_view = l.clone();
    let child_op = OutPoint::new(txid, 0);
    chained_view.insert(child_op, UtxoEntry::new(tx.output[0].clone(), 1, false));
    let child = b.spend(&chained_view, &[child_op], &a.1, 1_000);
    assert_eq!(admit(&mut pool, &l, child), Err(AdmitError::MissingInput(0)));

    // Fee below the minimum rate (10 per 1 000 vbytes).
    let cheap = a.spend(&l, &[op(2)], &b.1, 1);
    assert!(matches!(admit(&mut pool, &l, cheap), Err(AdmitError::FeeTooLow { .. })));

    // Invalid transactions are remembered.
    let mut bad = a.spend(&l, &[op(3)], &b.1, 1_000);
    bad.output[0] = out(&b.1, 100_000); // breaks the balance
    assert!(matches!(admit(&mut pool, &l, bad.clone()), Err(AdmitError::Invalid(_))));
    assert_eq!(admit(&mut pool, &l, bad), Err(AdmitError::KnownInvalid));

    assert_eq!(pool.len(), 1);
}

#[test]
fn template_orders_by_fee_rate_and_block_evicts() {
    let (a, b) = (Key::new(1), Key::new(2));
    let l = setup(&a, 3);
    let mut pool = Mempool::new(Policy::new(asset()));
    let low = admit(&mut pool, &l, a.spend(&l, &[op(1)], &b.1, 1_000)).unwrap();
    let high = admit(&mut pool, &l, a.spend(&l, &[op(2)], &b.1, 5_000)).unwrap();
    let mid = admit(&mut pool, &l, a.spend(&l, &[op(3)], &b.1, 3_000)).unwrap();
    let order: Vec<Txid> = pool.template(usize::MAX).iter().map(|t| t.txid()).collect();
    assert_eq!(order, vec![high, mid, low]);

    // A block containing `mid` and a different spend of op(1) removes `mid` and `low`.
    let mid_tx = (*pool.get(&mid).unwrap().tx).clone();
    let rival = a.spend(&l, &[op(1)], &a.1, 2_000);
    pool.remove_for_block(&[mid_tx, rival]);
    assert_eq!(pool.txids(), vec![high]);

    pool.expire(Instant::now() + Duration::from_secs(25 * 3600));
    assert!(pool.is_empty());
}

#[test]
fn wire_batch_round_trip() {
    let (a, b) = (Key::new(1), Key::new(2));
    let l = setup(&a, 2);
    let txs = vec![a.spend(&l, &[op(1)], &b.1, 1_000), a.spend(&l, &[op(2)], &b.1, 2_000)];
    let bytes = wire::encode_batch(&txs);
    assert_eq!(wire::decode_batch(&bytes).unwrap(), txs);
    assert!(wire::decode_batch(&bytes[..bytes.len() - 1]).is_none());
    let mut extra = bytes.clone();
    extra.push(0);
    assert!(wire::decode_batch(&extra).is_none());
}

#[test]
fn cheap_policy_checks_run_before_verification() {
    let (a, b) = (Key::new(1), Key::new(2));
    let l = setup(&a, 2);
    let mut pool = Mempool::new(Policy::new(asset()));

    // Bad signature and too low a fee: the fee is reported (no signature work done).
    let mut tx = a.spend(&l, &[op(1)], &b.1, 1);
    tx.input[0].witness.script_witness = Witness::from_slice(&[vec![1u8; 64]]);
    assert!(matches!(admit(&mut pool, &l, tx), Err(AdmitError::FeeTooLow { .. })));

    // Fees in another asset are refused by policy (the coinbase only claims the fee asset).
    let other = AssetId::from_byte_array([8; 32]);
    let mut l2 = setup(&a, 1);
    l2.insert(op(9), UtxoEntry::new(TxOut { asset: Asset::Explicit(other), ..out(&a.1, 100_000) }, 1, false));
    let mut tx = a.spend(&l2, &[op(1)], &b.1, 1_000);
    tx.input.push(TxIn { previous_output: op(9), sequence: Sequence::MAX, ..Default::default() });
    tx.output.push(TxOut::new_fee(100_000, other));
    assert_eq!(admit(&mut pool, &l2, tx), Err(AdmitError::FeeAsset(other)));
}

#[test]
fn split_admission_rechecks_conflicts_on_insert() {
    // Two spends of one coin verified concurrently: only the first insert wins.
    let (a, b, c) = (Key::new(1), Key::new(2), Key::new(3));
    let l = setup(&a, 1);
    let mut pool = Mempool::new(Policy::new(asset()));
    let cache = VerifyCache::new();
    let first = pool.precheck(a.spend(&l, &[op(1)], &b.1, 1_000), Source::Local, &l).unwrap();
    let second = pool.precheck(a.spend(&l, &[op(1)], &c.1, 2_000), Source::Local, &l).unwrap();
    wharfedale_mempool::verify(&first, &l, &ctx(), &cache).unwrap();
    wharfedale_mempool::verify(&second, &l, &ctx(), &cache).unwrap();
    let txid = pool.insert(first, &l, Instant::now()).unwrap();
    assert_eq!(pool.insert(second, &l, Instant::now()), Err(AdmitError::Conflict(0, txid)));
    assert_eq!(pool.len(), 1);
}
