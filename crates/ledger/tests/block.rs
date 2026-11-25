//! `connect_block` rules: the optional coinbase, ordering, fee claim, UTXO diff.

mod common;

use common::*;
use elements::{OutPoint, Script};
use wharfedale_ledger::{connect_block, BlockError, LedgerView, TxError};

fn spend(ledger: &impl LedgerView, from: OutPoint, key: &Key, to: &Key, value: u64, fee_value: u64) -> elements::Transaction {
    sign_all(ledger, unsigned(&[from], vec![explicit(&to.spk, value, asset()), fee(fee_value)]), key)
}

#[test]
fn valid_block_produces_diff() {
    let (a, b, p) = (Key::new(1), Key::new(2), Key::new(3));
    let mut ledger = ledger_with(&[(1, &a)]);
    let tx = spend(&ledger, outpoint(1), &a, &b, 99_000, 1_000);
    let cb = coinbase(HEIGHT, vec![explicit(&p.spk, 1_000, asset())]);
    let diff = connect_block(&ledger, &[cb.clone(), tx.clone()], &ctx()).unwrap();

    assert_eq!(diff.fees.get(&asset()), Some(&1_000));
    assert_eq!(diff.spent.iter().map(|(op, _)| *op).collect::<Vec<_>>(), vec![outpoint(1)]);
    let created: Vec<OutPoint> = diff.created.iter().map(|(op, _)| *op).collect();
    assert_eq!(created, vec![OutPoint::new(cb.txid(), 0), OutPoint::new(tx.txid(), 0)]);
    assert!(diff.created[0].1.is_coinbase);

    ledger.apply(&diff);
    assert_eq!(ledger.len(), 2);
    assert!(ledger.utxo(&outpoint(1)).is_none());
}

#[test]
fn coinbase_may_not_overclaim() {
    let (a, b, p) = (Key::new(1), Key::new(2), Key::new(3));
    let ledger = ledger_with(&[(1, &a)]);
    let tx = spend(&ledger, outpoint(1), &a, &b, 99_000, 1_000);
    let cb = coinbase(HEIGHT, vec![explicit(&p.spk, 1_001, asset())]);
    assert_eq!(
        connect_block(&ledger, &[cb, tx], &ctx()),
        Err(BlockError::CoinbaseOverclaim(asset()))
    );
}

#[test]
fn empty_block_with_op_return_coinbase_is_valid() {
    let cb = coinbase(HEIGHT, vec![explicit(&Script::new_op_return(b""), 0, asset())]);
    let diff = connect_block(&ledger_with(&[]), &[cb], &ctx()).unwrap();
    assert!(diff.created.is_empty() && diff.spent.is_empty() && diff.fees.is_empty());
}

#[test]
fn coinbase_rules() {
    let p = Key::new(3);
    let ledger = ledger_with(&[]);
    let run = |txs: &[elements::Transaction]| connect_block(&ledger, txs, &ctx());

    // No transactions at all: valid (the proposer's policy is what keeps blocks from being empty).
    let empty = run(&[]).unwrap();
    assert!(empty.created.is_empty() && empty.spent.is_empty() && empty.fees.is_empty());
    assert!(matches!(run(&[coinbase(HEIGHT + 1, vec![explicit(&p.spk, 0, asset())])]), Err(BlockError::Coinbase(_))));

    let mut no_pad = coinbase(HEIGHT, vec![]);
    no_pad.output.push(explicit(&Script::new_op_return(b""), 0, asset()));
    no_pad.input[0].script_sig = Script::from(wharfedale_ledger::coinbase_height_prefix(HEIGHT));
    assert!(matches!(run(&[no_pad]), Err(BlockError::Coinbase(_))), "script_sig shorter than 2 bytes");

    let zero = coinbase(HEIGHT, vec![explicit(&p.spk, 0, asset())]);
    assert!(matches!(run(&[zero]), Err(BlockError::Coinbase(_))), "zero-value spendable output");

    let fee_out = coinbase(HEIGHT, vec![fee(1)]);
    assert!(matches!(run(&[fee_out]), Err(BlockError::Coinbase(_))), "fee output kind");
}

#[test]
fn block_without_coinbase_burns_its_fees() {
    let (a, b) = (Key::new(1), Key::new(2));
    let ledger = ledger_with(&[(1, &a)]);
    let tx = spend(&ledger, outpoint(1), &a, &b, 99_000, 1_000);
    let diff = connect_block(&ledger, std::slice::from_ref(&tx), &ctx()).unwrap();
    assert_eq!(diff.fees.get(&asset()), Some(&1_000));
    let created: Vec<OutPoint> = diff.created.iter().map(|(op, _)| *op).collect();
    assert_eq!(created, vec![OutPoint::new(tx.txid(), 0)], "only the payment: nobody receives the fee");
    assert!(diff.created.iter().all(|(_, e)| !e.is_coinbase));
}

#[test]
fn coinbase_must_come_first() {
    let (a, b, p) = (Key::new(1), Key::new(2), Key::new(3));
    let ledger = ledger_with(&[(1, &a)]);
    let tx = spend(&ledger, outpoint(1), &a, &b, 99_000, 1_000);
    let cb = coinbase(HEIGHT, vec![explicit(&p.spk, 1_000, asset())]);
    assert_eq!(connect_block(&ledger, &[tx, cb], &ctx()), Err(BlockError::ExtraCoinbase(1)));
}

#[test]
fn transaction_index_counts_from_zero_without_coinbase() {
    let (a, b, c) = (Key::new(1), Key::new(2), Key::new(4));
    let ledger = ledger_with(&[(1, &a)]);
    let tx1 = spend(&ledger, outpoint(1), &a, &b, 99_000, 1_000);
    let tx2 = spend(&ledger, outpoint(1), &a, &c, 99_000, 1_000);
    let err = connect_block(&ledger, &[tx1, tx2.clone()], &ctx()).unwrap_err();
    assert_eq!(err, BlockError::Tx { index: 1, txid: tx2.txid(), error: TxError::MissingInput(0, outpoint(1)) });
}

#[test]
fn second_coinbase_is_rejected() {
    let p = Key::new(3);
    let cb = coinbase(HEIGHT, vec![explicit(&Script::new_op_return(b""), 0, asset())]);
    let mut cb2 = coinbase(HEIGHT, vec![explicit(&p.spk, 1, asset())]);
    cb2.input[0].script_sig = Script::from(vec![0x5a, 0x01]);
    assert_eq!(
        connect_block(&ledger_with(&[]), &[cb, cb2], &ctx()),
        Err(BlockError::ExtraCoinbase(1))
    );
}

#[test]
fn chained_spend_within_block() {
    let (a, b, c, p) = (Key::new(1), Key::new(2), Key::new(4), Key::new(3));
    let ledger = ledger_with(&[(1, &a)]);
    let tx1 = spend(&ledger, outpoint(1), &a, &b, 99_000, 1_000);

    let mut after_tx1 = ledger.clone();
    after_tx1.insert(
        OutPoint::new(tx1.txid(), 0),
        wharfedale_ledger::UtxoEntry::new(tx1.output[0].clone(), HEIGHT, false),
    );
    let tx2 = spend(&after_tx1, OutPoint::new(tx1.txid(), 0), &b, &c, 98_000, 1_000);

    let cb = coinbase(HEIGHT, vec![explicit(&p.spk, 2_000, asset())]);
    let diff = connect_block(&ledger, &[cb.clone(), tx1, tx2.clone()], &ctx()).unwrap();
    let created: Vec<OutPoint> = diff.created.iter().map(|(op, _)| *op).collect();
    assert_eq!(created, vec![OutPoint::new(cb.txid(), 0), OutPoint::new(tx2.txid(), 0)]);
    assert_eq!(diff.spent.len(), 1);
    assert_eq!(diff.fees.get(&asset()), Some(&2_000));
}

#[test]
fn double_spend_within_block_is_rejected() {
    let (a, b, c) = (Key::new(1), Key::new(2), Key::new(4));
    let ledger = ledger_with(&[(1, &a)]);
    let tx1 = spend(&ledger, outpoint(1), &a, &b, 99_000, 1_000);
    let tx2 = spend(&ledger, outpoint(1), &a, &c, 99_000, 1_000);
    let cb = coinbase(HEIGHT, vec![explicit(&Script::new_op_return(b""), 0, asset())]);
    let err = connect_block(&ledger, &[cb, tx1, tx2.clone()], &ctx()).unwrap_err();
    assert_eq!(
        err,
        BlockError::Tx { index: 2, txid: tx2.txid(), error: TxError::MissingInput(0, outpoint(1)) }
    );
}

#[test]
fn coinbase_output_is_immature_in_its_own_block() {
    let (p, b) = (Key::new(3), Key::new(2));
    let ledger = ledger_with(&[]);
    let cb = coinbase(HEIGHT, vec![explicit(&p.spk, 1, asset()), explicit(&Script::new_op_return(b""), 0, asset())]);
    let mut view = ledger.clone();
    view.insert(
        OutPoint::new(cb.txid(), 0),
        wharfedale_ledger::UtxoEntry::new(cb.output[0].clone(), HEIGHT, true),
    );
    let tx = sign_all(&view, unsigned(&[OutPoint::new(cb.txid(), 0)], vec![explicit(&b.spk, 1, asset())]), &p);
    let err = connect_block(&ledger, &[cb, tx.clone()], &ctx()).unwrap_err();
    assert_eq!(err, BlockError::Tx { index: 1, txid: tx.txid(), error: TxError::ImmatureCoinbase(0) });
}
