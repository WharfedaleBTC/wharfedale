//! Test fixtures: keys, explicit outputs, signed transactions, coinbases.
#![allow(dead_code)]

use elements::confidential::{Asset, Nonce, Value};
use elements::schnorr::{TapTweak, TweakedKeypair};
use elements::secp256k1_zkp::{Keypair, Message, SECP256K1};
use elements::sighash::{Prevouts, SighashCache};
use elements::{
    AssetId, BlockHash, LockTime, OutPoint, SchnorrSighashType, Script, Sequence, Transaction,
    TxIn, TxOut, TxOutWitness, Txid, Witness,
};
use wharfedale_ledger::{coinbase_height_prefix, ChainCtx, LedgerView, MemoryLedger, UtxoEntry};

pub const HEIGHT: u64 = 10;
pub const TIME: u64 = 1_800_000_000; // block timestamp, unix seconds, above LOCKTIME_THRESHOLD

pub fn genesis() -> BlockHash {
    BlockHash::from_byte_array([9; 32])
}

pub fn asset() -> AssetId {
    AssetId::from_byte_array([7; 32])
}

pub fn other_asset() -> AssetId {
    AssetId::from_byte_array([8; 32])
}

pub fn ctx() -> ChainCtx {
    ChainCtx { height: HEIGHT, time: TIME, genesis_hash: genesis(), coinbase_maturity: 1 }
}

/// A taproot key-path-only owner.
pub struct Key {
    tweaked: TweakedKeypair,
    pub spk: Script,
}

impl Key {
    pub fn new(seed: u8) -> Self {
        let kp = Keypair::from_seckey_slice(SECP256K1, &[seed; 32]).unwrap();
        let (internal, _) = kp.x_only_public_key();
        Self {
            tweaked: kp.tap_tweak(SECP256K1, None),
            spk: Script::new_v1_p2tr(SECP256K1, internal, None),
        }
    }

    pub fn sign(&self, tx: &Transaction, ix: usize, prevouts: &[TxOut], ty: SchnorrSighashType) -> Vec<u8> {
        let hash = SighashCache::new(tx)
            .taproot_key_spend_signature_hash(ix, &Prevouts::All(prevouts), ty, genesis())
            .unwrap();
        let sig = SECP256K1
            .sign_schnorr_no_aux_rand(&Message::from_digest(hash.to_byte_array()), &self.tweaked.to_inner());
        let mut bytes = sig.serialize().to_vec();
        if ty != SchnorrSighashType::Default {
            bytes.push(ty as u8);
        }
        bytes
    }
}

pub fn explicit(spk: &Script, value: u64, asset: AssetId) -> TxOut {
    TxOut {
        asset: Asset::Explicit(asset),
        value: Value::Explicit(value),
        nonce: Nonce::Null,
        script_pubkey: spk.clone(),
        witness: TxOutWitness::empty(),
    }
}

pub fn fee(value: u64) -> TxOut {
    TxOut::new_fee(value, asset())
}

pub fn outpoint(n: u8) -> OutPoint {
    OutPoint::new(Txid::from_byte_array([n; 32]), 0)
}

/// A ledger holding one 100 000-unit output of `asset()` per `(outpoint n, owner)`.
pub fn ledger_with(owners: &[(u8, &Key)]) -> MemoryLedger {
    let mut ledger = MemoryLedger::new();
    for (n, key) in owners {
        ledger.insert(outpoint(*n), UtxoEntry::new(explicit(&key.spk, 100_000, asset()), 1, false));
    }
    ledger
}

pub fn unsigned(inputs: &[OutPoint], outputs: Vec<TxOut>) -> Transaction {
    Transaction {
        version: 2,
        lock_time: LockTime::ZERO,
        input: inputs
            .iter()
            .map(|op| TxIn { previous_output: *op, sequence: Sequence::MAX, ..Default::default() })
            .collect(),
        output: outputs,
    }
}

/// Signs every input with `key` (SIGHASH_DEFAULT), reading prevouts from `view`.
pub fn sign_all(view: &impl LedgerView, mut tx: Transaction, key: &Key) -> Transaction {
    let prevouts: Vec<TxOut> =
        tx.input.iter().map(|i| view.utxo(&i.previous_output).unwrap().txout).collect();
    for ix in 0..tx.input.len() {
        let sig = key.sign(&tx, ix, &prevouts, SchnorrSighashType::Default);
        tx.input[ix].witness.script_witness = Witness::from_slice(&[sig]);
    }
    tx
}

pub fn coinbase(height: u64, outputs: Vec<TxOut>) -> Transaction {
    let mut script_sig = coinbase_height_prefix(height);
    script_sig.push(0x00); // pad: script_sig must be at least 2 bytes
    Transaction {
        version: 2,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            script_sig: Script::from(script_sig),
            sequence: Sequence::MAX,
            ..Default::default()
        }],
        output: outputs,
    }
}
