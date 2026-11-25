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

/// Writes a key file readable by its owner only (mode 0600 on Unix).
pub fn save(path: impl AsRef<Path>, secret: &[u8; 32]) -> Result<()> {
    use std::io::Write as _;
    let file = KeyFile { secret: hex::encode(secret) };
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut f = options.open(path.as_ref())?;
    // `mode` applies only to a new file: tighten an existing one before writing the secret.
    #[cfg(unix)]
    f.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    f.write_all(&serde_json::to_vec_pretty(&file)?)?;
    f.sync_all()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_files_round_trip_and_are_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("k.json");
        std::fs::write(&path, b"old").unwrap(); // an existing, more permissive file is tightened
        save(&path, &[9; 32]).unwrap();
        assert_eq!(load(&path).unwrap(), [9; 32]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }
}
