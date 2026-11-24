//! Network parameters: address format, fee-asset derivation, genesis funding.

use elements::bitcoin::bech32::Hrp;
use elements::confidential::{Asset, Nonce, Value};
use elements::hashes::sha256;
use elements::issuance::ContractHash;
use elements::secp256k1_zkp::{Keypair, SECP256K1};
use elements::script::Builder;
use elements::{AddressParams, AssetId, LockTime, OutPoint, Script, Sequence, Transaction, TxIn, TxOut, TxOutWitness, Txid};

/// Wharfedale address parameters: `wf1…` (explicit segwit/taproot), `wfc1…` (confidential).
/// Base58 prefixes are unused (no legacy outputs) but must be set.
pub static ADDRESS_PARAMS: AddressParams = AddressParams {
    p2pkh_prefix: 73,
    p2sh_prefix: 87,
    blinded_prefix: 25,
    bech_hrp: Hrp::parse_unchecked("wf"),
    blech_hrp: Hrp::parse_unchecked("wfc"),
};

/// The fee asset of a chain: an Elements issuance from outpoint `0000…:0` with contract hash
/// SHA-256(chain_id). Derived, so it is identical on every node and in every wallet.
pub fn fee_asset_for(chain_id: &str) -> AssetId {
    let contract = ContractHash::from_byte_array(sha256::Hash::hash(chain_id.as_bytes()).to_byte_array());
    AssetId::new_issuance(OutPoint::new(Txid::from_byte_array([0; 32]), 0), contract)
}

/// A genesis transaction paying explicit `outputs` of `asset`. It has a null-prevout input and
/// is applied without validation (genesis only).
pub fn genesis_funding_tx(asset: AssetId, outputs: &[(Script, u64)]) -> Transaction {
    Transaction {
        version: 2,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            script_sig: Builder::new().push_slice(b"wharfedale genesis").into_script(),
            sequence: Sequence::MAX,
            ..Default::default()
        }],
        output: outputs
            .iter()
            .map(|(spk, value)| TxOut {
                asset: Asset::Explicit(asset),
                value: Value::Explicit(*value),
                nonce: Nonce::Null,
                script_pubkey: spk.clone(),
                witness: TxOutWitness::empty(),
            })
            .collect(),
    }
}

/// Key-path-only taproot output script for a 32-byte secret (the wallet's and the testnet
/// tooling's single-key address). Panics on an invalid secret (probability ~2^-128).
pub fn key_path_script(secret: &[u8; 32]) -> Script {
    let kp = Keypair::from_seckey_slice(SECP256K1, secret).expect("valid secret key");
    Script::new_v1_p2tr(SECP256K1, kp.x_only_public_key().0, None)
}
