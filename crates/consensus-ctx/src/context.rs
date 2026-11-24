//! The Malachite `Context` for Wharfedale.

use bytes::Bytes;
use malachitebft_core_types::{Context, LinearTimeouts, NilOrVal, Round, ValidatorSet as _};

use crate::{Address, Ed25519, Height, Proposal, ProposalPart, Validator, ValidatorSet, Value, ValueId, Vote};

#[derive(Clone, Copy, Debug, Default)]
pub struct WharfedaleContext;

impl WharfedaleContext {
    /// Round-robin over the validator set (sorted by address): `(height - 1 + round) mod n`.
    pub fn proposer(validator_set: &ValidatorSet, height: Height, round: Round) -> &Validator {
        assert!(validator_set.count() > 0);
        assert!(round != Round::Nil && round.as_i64() >= 0);
        let index = (height.as_u64() as usize - 1 + round.as_i64() as usize) % validator_set.count();
        validator_set.get_by_index(index).expect("index < count")
    }
}

impl Context for WharfedaleContext {
    type Address = Address;
    type ProposalPart = ProposalPart;
    type Height = Height;
    type Proposal = Proposal;
    type ValidatorSet = ValidatorSet;
    type Validator = Validator;
    type Timeouts = LinearTimeouts;
    type Value = Value;
    type Vote = Vote;
    type Extension = Bytes;
    type SigningScheme = Ed25519;

    fn select_proposer<'a>(&self, validator_set: &'a ValidatorSet, height: Height, round: Round) -> &'a Validator {
        Self::proposer(validator_set, height, round)
    }

    fn new_proposal(&self, height: Height, round: Round, value: Value, pol_round: Round, address: Address) -> Proposal {
        Proposal::new(height, round, value, pol_round, address)
    }

    fn new_prevote(&self, height: Height, round: Round, value_id: NilOrVal<ValueId>, address: Address) -> Vote {
        Vote::new_prevote(height, round, value_id, address)
    }

    fn new_precommit(&self, height: Height, round: Round, value_id: NilOrVal<ValueId>, address: Address) -> Vote {
        Vote::new_precommit(height, round, value_id, address)
    }
}
