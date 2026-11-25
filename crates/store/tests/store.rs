//! Genesis init/check, atomic commit, reopen, in-flight pruning, LedgerView.

use elements::{BlockHash, OutPoint, Txid};
use wharfedale_chain::{merkle, Block, Genesis, GenesisValidator, Header};
use wharfedale_ledger::{BlockDiff, LedgerView, UtxoEntry};
use wharfedale_store::{Store, StoreError};

fn genesis(chain_id: &str) -> Genesis {
    Genesis {
        chain_id: chain_id.into(),
        genesis_time: 1_800_000_000,
        validators: vec![GenesisValidator { public_key: hex::encode([1u8; 32]), voting_power: 1 }],
        fee_asset: wharfedale_chain::fee_asset_for(chain_id).to_string(),
        initial_txs: vec![],
    }
}

fn next_block(parent: &Header) -> Block {
    Block {
        header: Header {
            version: Header::VERSION,
            height: parent.height + 1,
            prev_block_hash: parent.hash().to_byte_array(),
            timestamp: parent.timestamp + 1,
            proposer: [0; 20],
            tx_merkle_root: merkle::tx_root(&[]),
            witness_merkle_root: merkle::witness_root(&[]),
        },
        txs: vec![],
    }
}

fn entry() -> UtxoEntry {
    let spk = elements::Script::from(vec![0x51, 0x20].into_iter().chain([3u8; 32]).collect::<Vec<u8>>());
    UtxoEntry::new(elements::TxOut::new_fee(5, elements::AssetId::from_byte_array([7; 32])), 1, false)
        .with_script(spk)
}

trait WithScript {
    fn with_script(self, spk: elements::Script) -> Self;
}
impl WithScript for UtxoEntry {
    fn with_script(mut self, spk: elements::Script) -> Self {
        self.txout.script_pubkey = spk;
        self
    }
}

#[test]
fn genesis_commit_reopen_and_prune() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.redb");
    let op = OutPoint::new(Txid::from_byte_array([1; 32]), 0);
    let block1;
    {
        let store = Store::open(&path).unwrap();
        store.init_or_check(&genesis("a")).unwrap();
        let g = store.header(0).unwrap().unwrap();
        assert_eq!(store.tip().unwrap(), (0, g.hash()));

        block1 = next_block(&g);
        let hash1 = block1.hash();
        store.put_undecided(1, 0, &hash1, b"proposal").unwrap();
        store.put_built(1, 0, &hash1).unwrap();
        store.put_undecided(2, 0, &BlockHash::from_byte_array([2; 32]), b"later").unwrap();
        assert_eq!(store.undecided_by_hash(1, &hash1).unwrap().as_deref(), Some(&b"proposal"[..]));

        let diff = BlockDiff { created: vec![(op, entry())], ..Default::default() };
        store.commit(&block1, b"cert-1", &diff).unwrap();

        // Height 1 in-flight records are pruned; height 2 is kept.
        assert!(store.undecided_by_hash(1, &hash1).unwrap().is_none());
        assert!(store.built(1, 0).unwrap().is_none());
        assert_eq!(store.undecided_in_round(2, 0).unwrap().len(), 1);

        // Not the next block → refused.
        assert!(matches!(store.commit(&block1, b"x", &BlockDiff::default()), Err(StoreError::NotNext { .. })));
    }
    // Reopen: state persisted.
    let store = Store::open(&path).unwrap();
    store.init_or_check(&genesis("a")).unwrap();
    assert_eq!(store.tip().unwrap(), (1, block1.hash()));
    assert_eq!(store.cert(1).unwrap().as_deref(), Some(&b"cert-1"[..]));
    assert_eq!(store.height_of(&block1.hash()).unwrap(), Some(1));
    assert_eq!(store.utxo(&op), Some(entry()));
    assert_eq!(store.block_time(1), block1.header.timestamp);
    // A different genesis is rejected.
    assert!(matches!(store.init_or_check(&genesis("b")), Err(StoreError::WrongChain)));
}

mod wfa1 {
    use elements::confidential::{Asset, Nonce, Value};
    use elements::hashes::sha256;
    use elements::script::Builder;
    use elements::{opcodes, AssetIssuance, OutPoint, Script, Transaction, TxIn, TxOut, TxOutWitness, Txid};
    use wharfedale_chain::{merkle, Block, Header};
    use wharfedale_ledger::BlockDiff;
    use wharfedale_store::{parse_asset_metadata, wfa1_payload, AssetMetadata, Store};

    const META: &[u8] = br#"{"name":"  Gold Coin ","ticker":"GLD","precision":2,"extra":[1]}"#;

    fn wfa1_script(meta: &[u8]) -> Script {
        Builder::new().push_opcode(opcodes::all::OP_RETURN).push_slice(b"WFA1").push_slice(meta).into_script()
    }

    fn data_out(script: Script) -> TxOut {
        let asset = Asset::Explicit(wharfedale_chain::fee_asset_for("a"));
        TxOut { asset, value: Value::Explicit(0), nonce: Nonce::Null, script_pubkey: script, witness: TxOutWitness::default() }
    }

    fn taproot_out() -> TxOut {
        let spk = Script::from([0x51, 0x20].into_iter().chain([3u8; 32]).collect::<Vec<u8>>());
        TxOut { script_pubkey: spk, ..data_out(Script::new()) }
    }

    /// A new issuance spending `(seed, 0)` with contract hash `contract`.
    fn issuance(seed: u8, contract: [u8; 32], outputs: Vec<TxOut>) -> Transaction {
        let input = TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([seed; 32]), 0),
            asset_issuance: AssetIssuance {
                asset_blinding_nonce: elements::AssetBlindingNonce::NEW_ISSUANCE,
                asset_entropy: elements::AssetEntropy::from_byte_array(contract),
                amount: Value::Explicit(1_000),
                inflation_keys: Value::Explicit(1),
            },
            ..Default::default()
        };
        Transaction { version: 2, lock_time: elements::LockTime::ZERO, input: vec![input], output: outputs }
    }

    fn commit(store: &Store, txs: Vec<Transaction>) {
        let (tip, _) = store.tip().unwrap();
        let parent = store.header(tip).unwrap().unwrap();
        let block = Block {
            header: Header {
                version: Header::VERSION,
                height: tip + 1,
                prev_block_hash: parent.hash().to_byte_array(),
                timestamp: parent.timestamp + 1,
                proposer: [0; 20],
                tx_merkle_root: merkle::tx_root(&txs),
                witness_merkle_root: merkle::witness_root(&txs),
            },
            txs,
        };
        store.commit(&block, b"cert", &BlockDiff::default()).unwrap();
        while store.index_next().unwrap().is_some() {}
    }

    fn sha(b: &[u8]) -> [u8; 32] {
        sha256::Hash::hash(b).to_byte_array()
    }

    #[test]
    fn issuance_names_from_wfa1_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("store.redb")).unwrap();
        store.init_or_check(&super::genesis("a")).unwrap();

        let other = br#"{"name":"Silver","ticker":"SLV","precision":0}"#;
        let bad_ticker = br#"{"name":"Lead","ticker":"lead","precision":0}"#;
        let named = issuance(1, sha(META), vec![taproot_out(), data_out(wfa1_script(META))]);
        // The output names another contract: no name.
        let mismatched = issuance(2, sha(META), vec![data_out(wfa1_script(other)), taproot_out()]);
        let unnamed = issuance(3, sha(META), vec![taproot_out()]);
        let invalid = issuance(4, sha(bad_ticker), vec![data_out(wfa1_script(bad_ticker))]);
        // Matching metadata, but a confidential-value output is not a WFA1 output.
        let mut blinded = data_out(wfa1_script(other));
        blinded.value = Value::Null;
        let not_explicit = issuance(5, sha(other), vec![blinded, taproot_out()]);
        commit(&store, vec![named.clone(), mismatched.clone(), unnamed.clone(), invalid.clone(), not_explicit.clone()]);

        let record = |tx: &Transaction| store.asset(&tx.input[0].issuance_ids().0).unwrap().unwrap();
        let r = record(&named);
        assert_eq!((r.name.as_deref(), r.ticker.as_deref(), r.precision), (Some("Gold Coin"), Some("GLD"), Some(2)));
        assert_eq!(r.contract_hash, hex::encode(sha(META)));
        for tx in [&mismatched, &unnamed, &invalid, &not_explicit] {
            let r = record(tx);
            assert_eq!((r.name, r.ticker, r.precision), (None, None, None), "{}", tx.txid());
        }

        // A reissuance (with its own WFA1 output) keeps the name.
        let r = record(&named);
        let mut reissue = issuance(6, [0; 32], vec![data_out(wfa1_script(other)), taproot_out()]);
        let iss = &mut reissue.input[0].asset_issuance;
        iss.asset_blinding_nonce = elements::AssetBlindingNonce::from_byte_array([7; 32]);
        iss.asset_entropy = elements::AssetEntropy::from_byte_array(hex::decode(&r.entropy).unwrap().try_into().unwrap());
        assert_eq!(reissue.input[0].issuance_ids().0, named.input[0].issuance_ids().0);
        commit(&store, vec![reissue]);
        let r = record(&named);
        assert_eq!(r.issuance_count, 2);
        assert_eq!((r.name.as_deref(), r.ticker.as_deref(), r.precision), (Some("Gold Coin"), Some("GLD"), Some(2)));

        // Records written before the name fields existed still decode.
        let mut old = serde_json::to_value(&r).unwrap();
        for k in ["name", "ticker", "precision"] {
            old.as_object_mut().unwrap().remove(k);
        }
        let old: wharfedale_store::AssetRecord = serde_json::from_value(old).unwrap();
        assert_eq!(old.name, None);
    }

    #[test]
    fn wfa1_script_and_metadata_rules() {
        assert_eq!(wfa1_payload(&wfa1_script(META)), Some(META));
        let long = [b' '; 128];
        assert_eq!(wfa1_payload(&wfa1_script(&long)), Some(&long[..]), "OP_PUSHDATA1");
        assert_eq!(wfa1_payload(&wfa1_script(&[b' '; 129])), None, "over 128 bytes");
        // Non-minimal push, extra push, other tag.
        let mut non_minimal = vec![0x6a, 0x04, b'W', b'F', b'A', b'1', 0x4c, 2];
        non_minimal.extend_from_slice(b"{}");
        assert_eq!(wfa1_payload(&Script::from(non_minimal)), None);
        let extra = Builder::from(wfa1_script(META).into_bytes()).push_slice(b"x").into_script();
        assert_eq!(wfa1_payload(&extra), None);
        let other_tag = Builder::new().push_opcode(opcodes::all::OP_RETURN).push_slice(b"WFA2").push_slice(META).into_script();
        assert_eq!(wfa1_payload(&other_tag), None);

        let ok = |m: &str| parse_asset_metadata(m.as_bytes());
        assert_eq!(
            ok(r#"{"name":"A","ticker":"Z9","precision":8}"#),
            Some(AssetMetadata { name: "A".into(), ticker: "Z9".into(), precision: 8 })
        );
        let name64 = "n".repeat(64);
        assert!(ok(&format!(r#"{{"name":"{name64}","ticker":"T","precision":0}}"#)).is_some());
        for bad in [
            r#"{"name":"   ","ticker":"T","precision":0}"#,
            r#"{"name":"a\u0007b","ticker":"T","precision":0}"#,
            r#"{"name":"A","ticker":"TOOLONGXX","precision":0}"#,
            r#"{"name":"A","ticker":"","precision":0}"#,
            r#"{"name":"A","ticker":"T-1","precision":0}"#,
            r#"{"name":"A","ticker":"T","precision":9}"#,
            r#"{"name":"A","ticker":"T","precision":1.5}"#,
            r#"{"name":"A","ticker":"T","precision":-1}"#,
            r#"{"name":"A","ticker":"T"}"#,
            r#"["A","T",0]"#,
            "not json",
        ] {
            assert_eq!(ok(bad), None, "{bad}");
        }
        assert!(ok(&format!(r#"{{"name":"{}","ticker":"T","precision":0}}"#, "n".repeat(65))).is_none());
    }
}
