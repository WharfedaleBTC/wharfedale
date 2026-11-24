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
    assert_eq!(store.median_time_past(1), 1_800_000_001);
    // A different genesis is rejected.
    assert!(matches!(store.init_or_check(&genesis("b")), Err(StoreError::WrongChain)));
}
