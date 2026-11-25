//! The Malachite `Context` for Wharfedale.

use bytes::Bytes;
use malachitebft_core_types::{Context, LinearTimeouts, NilOrVal, Round, ValidatorSet as _};

use crate::{Address, Ed25519, Height, Proposal, ProposalPart, Validator, ValidatorSet, Value, ValueId, Vote};

#[derive(Clone, Copy, Debug, Default)]
pub struct WharfedaleContext;

impl WharfedaleContext {
    /// Round-robin over the validator set in genesis order: `(height - 1 + round) mod n`.
    /// Never overflows: heights and rounds can come from peers.
    pub fn proposer(validator_set: &ValidatorSet, height: Height, round: Round) -> &Validator {
        assert!(validator_set.count() > 0);
        assert!(round != Round::Nil && round.as_i64() >= 0);
        let n = validator_set.count() as u64;
        let index = (height.as_u64().saturating_sub(1) % n + round.as_i64() as u64 % n) % n;
        validator_set.get_by_index(index as usize).expect("index < count")
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

#[cfg(test)]
mod tests {
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    use super::*;
    use crate::PrivateKey;

    #[test]
    fn proposer_rotates_in_genesis_order_and_never_overflows() {
        let mut rng = StdRng::seed_from_u64(1);
        let set = ValidatorSet::new((0..3).map(|_| Validator::new(PrivateKey::generate(&mut rng).public_key(), 1)));
        let at = |h: u64, r: u32| WharfedaleContext::proposer(&set, Height::new(h), Round::new(r)).address;
        assert_eq!(at(1, 0), set.validators[0].address);
        assert_eq!(at(2, 0), set.validators[1].address);
        assert_eq!(at(1, 2), set.validators[2].address);
        assert_eq!(at(3, 1), set.validators[0].address);
        // Height 0 and extreme values (peer-supplied) don't panic.
        at(0, 0);
        at(u64::MAX, u32::MAX);
    }
}
