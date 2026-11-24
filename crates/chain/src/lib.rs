//! Block, header, genesis, merkle roots and their encoding.
//!
//! Spec: `docs/src/consensus/blocks.md`, `docs/src/configuration.md`.
#![forbid(unsafe_code)]

mod block;
mod genesis;
mod header;
pub mod merkle;
mod params;
mod validate;

pub use block::{Block, BlockDecodeError};
pub use genesis::{genesis_diff, Genesis, GenesisError, GenesisValidator};
pub use header::{Header, HEADER_SIZE};
pub use params::{fee_asset_for, genesis_funding_tx, key_path_script, ADDRESS_PARAMS};
pub use validate::{check_header, median_time_past, HeaderError, MTP_WINDOW};

use elements::hashes::sha256d;

/// A 32-byte hash.
pub type Hash32 = [u8; 32];

/// SHA-256d, the hash used for headers, merkle nodes and certificates.
pub fn sha256d(data: &[u8]) -> Hash32 {
    sha256d::Hash::hash(data).to_byte_array()
}
