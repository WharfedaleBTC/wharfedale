//! Header/block encoding, merkle roots, MTP, genesis determinism, header rules.

use wharfedale_chain::{check_header, median_time_past, merkle, Block, Genesis, GenesisValidator, Header, HeaderError};

fn genesis(chain_id: &str) -> Genesis {
    Genesis {
        chain_id: chain_id.into(),
        genesis_time: 1_800_000_000,
        validators: vec![GenesisValidator { public_key: hex::encode([1u8; 32]), voting_power: 1 }],
        fee_asset: wharfedale_chain::fee_asset_for(chain_id).to_string(),
        initial_txs: vec![],
    }
}

fn child(parent: &Header, proposer: [u8; 20], timestamp: u64) -> Header {
    Header {
        version: Header::VERSION,
        height: parent.height + 1,
        prev_block_hash: parent.hash().to_byte_array(),
        timestamp,
        proposer,
        tx_merkle_root: [0; 32],
        witness_merkle_root: [0; 32],
    }
}

#[test]
fn header_and_block_round_trip() {
    let block = genesis("a").block().unwrap();
    let bytes = block.encode();
    assert_eq!(Block::decode(&bytes).unwrap(), block);
    assert_eq!(Header::decode(&block.header.encode()).unwrap(), block.header);
    assert!(Block::decode(&bytes[..bytes.len() - 1]).is_err());
    let mut extra = bytes.clone();
    extra.push(0);
    assert!(Block::decode(&extra).is_err());
}

#[test]
fn genesis_hash_commits_to_chain_id_and_validators() {
    let a = genesis("a").block().unwrap().hash();
    assert_eq!(a, genesis("a").block().unwrap().hash(), "deterministic");
    assert_ne!(a, genesis("b").block().unwrap().hash());
    let mut other = genesis("a");
    other.validators[0].voting_power = 2;
    assert_ne!(a, other.block().unwrap().hash());
}

#[test]
fn merkle_root_rules() {
    assert_eq!(merkle::root(vec![]), [0; 32]);
    assert_eq!(merkle::root(vec![[5; 32]]), [5; 32]);
    // Odd levels duplicate the last node.
    assert_eq!(merkle::root(vec![[1; 32], [2; 32], [3; 32]]), merkle::root(vec![[1; 32], [2; 32], [3; 32], [3; 32]]));
    // ...which is why a duplicated subtree is detected as mutation.
    assert!(!merkle::root_checked(vec![[1; 32], [2; 32], [3; 32]]).1);
    assert!(merkle::root_checked(vec![[1; 32], [2; 32], [3; 32], [3; 32]]).1);
    assert!(merkle::root_checked(vec![[1; 32], [2; 32], [3; 32], [4; 32], [5; 32], [6; 32], [5; 32], [6; 32]]).1);
}

#[test]
fn median_time_past_uses_last_eleven() {
    assert_eq!(median_time_past(&[]), 0);
    assert_eq!(median_time_past(&[5, 1, 3]), 3);
    let times: Vec<u64> = (1..=20).collect();
    assert_eq!(median_time_past(&times), 15); // window 10..=20
}

#[test]
fn header_rules() {
    let parent = genesis("a").block().unwrap().header;
    let p = [7u8; 20];
    let now = parent.timestamp + 100;
    assert_eq!(check_header(&child(&parent, p, parent.timestamp + 1), &parent, &p, Some(now), 10), Ok(()));
    assert_eq!(
        check_header(&child(&parent, p, parent.timestamp), &parent, &p, Some(now), 10),
        Err(HeaderError::TimestampTooOld(parent.timestamp))
    );
    assert!(matches!(check_header(&child(&parent, p, now + 11), &parent, &p, Some(now), 10), Err(HeaderError::TimestampTooNew(_))));
    // Sync (decided blocks): no wall-clock check.
    assert_eq!(check_header(&child(&parent, p, now + 1_000), &parent, &p, None, 10), Ok(()));
    assert_eq!(check_header(&child(&parent, p, parent.timestamp + 1), &parent, &[8; 20], Some(now), 10), Err(HeaderError::Proposer));
    let mut wrong_prev = child(&parent, p, parent.timestamp + 1);
    wrong_prev.prev_block_hash = [9; 32];
    assert_eq!(check_header(&wrong_prev, &parent, &p, Some(now), 10), Err(HeaderError::PrevHash));
}

#[test]
fn duplicated_transactions_do_not_match_the_header() {
    let g = genesis("a").block().unwrap();
    let variant = |n: u32| {
        elements::Transaction { version: 2, lock_time: elements::LockTime::from_consensus(n), input: vec![], output: vec![] }
    };
    let txs = vec![variant(1), variant(2), variant(3)];
    let header = Header {
        tx_merkle_root: merkle::tx_root(&txs),
        witness_merkle_root: merkle::witness_root(&txs),
        ..g.header
    };
    let honest = Block { header: header.clone(), txs: txs.clone() };
    assert!(honest.roots_match());

    // Same roots and block hash, different body: rejected as mutated.
    let mut dup = txs;
    dup.push(variant(3));
    let mutated = Block { header: header.clone(), txs: dup };
    assert_eq!(merkle::tx_root(&mutated.txs), header.tx_merkle_root);
    assert_eq!(mutated.hash(), honest.hash());
    assert!(!mutated.roots_match());
}
