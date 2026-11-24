//! Key files: `{"secret": "<32-byte hex>"}`. The validator key signs consensus messages;
//! the node key is the libp2p identity.

use std::path::Path;

use eyre::{eyre, Result};
use rand::{CryptoRng, RngCore};
use serde::{Deserialize, Serialize};
use wharfedale_consensus_ctx::PrivateKey;

#[derive(Serialize, Deserialize)]
struct KeyFile {
    secret: String,
}

pub fn generate<R: RngCore + CryptoRng>(rng: &mut R) -> [u8; 32] {
    let mut secret = [0u8; 32];
    rng.fill_bytes(&mut secret);
    secret
}

pub fn save(path: impl AsRef<Path>, secret: &[u8; 32]) -> Result<()> {
    let file = KeyFile { secret: hex::encode(secret) };
    std::fs::write(path, serde_json::to_vec_pretty(&file)?)?;
    Ok(())
}

pub fn load(path: impl AsRef<Path>) -> Result<[u8; 32]> {
    let file: KeyFile = serde_json::from_slice(&std::fs::read(path.as_ref())?)?;
    hex::decode(&file.secret)?
        .try_into()
        .map_err(|_| eyre!("{}: secret must be 32 bytes", path.as_ref().display()))
}

pub fn private_key(secret: [u8; 32]) -> PrivateKey {
    PrivateKey::from(secret)
}
