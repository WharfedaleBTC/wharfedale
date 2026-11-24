//! Malachite `Context` implementation: Height, Address, Validator(Set), Vote, Proposal,
//! ProposalPart (block chunks), Value (= block hash), Ed25519 signing, protobuf codec.
//!
//! Most modules are adapted from Malachite `code/crates/test` (Apache-2.0); the value,
//! proposal-data and context modules are Wharfedale's. Spec: `docs/src/consensus/`.
#![forbid(unsafe_code)]

mod address;
pub mod codec;
mod context;
mod height;
mod proposal;
mod proposal_part;
mod signing;
mod validator_set;
mod value;
mod vote;

pub mod proto {
    #![allow(missing_docs, clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/wharfedale.rs"));
}

pub use address::*;
pub use context::*;
pub use height::*;
pub use proposal::*;
pub use proposal_part::*;
pub use signing::*;
pub use validator_set::*;
pub use value::*;
pub use vote::*;

pub use malachitebft_core_types::LinearTimeouts;

/// Builds the fixed validator set from `genesis.json` keys.
pub fn validator_set(keys: &[([u8; 32], u64)]) -> Result<ValidatorSet, String> {
    let validators = keys
        .iter()
        .map(|(key, power)| {
            PublicKey::from_bytes(*key)
                .map(|pk| Validator::new(pk, *power))
                .map_err(|e| format!("invalid validator key: {e}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ValidatorSet::new(validators))
}
