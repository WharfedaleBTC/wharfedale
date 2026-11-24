//! Header rules (`docs/src/consensus/blocks.md`) and median time past.

use crate::header::Header;

/// Number of blocks in the median-time-past window (BIP113).
pub const MTP_WINDOW: usize = 11;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum HeaderError {
    #[error("version {0} is not supported")]
    Version(u32),
    #[error("height {got}, expected {expected}")]
    Height { got: u64, expected: u64 },
    #[error("prev_block_hash does not match the tip")]
    PrevHash,
    #[error("proposer does not match the selected proposer")]
    Proposer,
    #[error("timestamp {0} is not after the parent's")]
    TimestampTooOld(u64),
    #[error("timestamp {0} is too far in the future")]
    TimestampTooNew(u64),
    #[error("merkle roots do not match the transactions")]
    MerkleRoots,
}

/// Checks a header against its parent. `now` is local unix time for a proposal (timestamp at
/// most `max_drift` seconds ahead), `None` for a decided block received by sync.
pub fn check_header(
    header: &Header,
    parent: &Header,
    expected_proposer: &[u8; 20],
    now: Option<u64>,
    max_drift: u64,
) -> Result<(), HeaderError> {
    if header.version != Header::VERSION {
        return Err(HeaderError::Version(header.version));
    }
    if header.height != parent.height + 1 {
        return Err(HeaderError::Height { got: header.height, expected: parent.height + 1 });
    }
    if header.prev_block_hash != parent.hash().to_byte_array() {
        return Err(HeaderError::PrevHash);
    }
    if &header.proposer != expected_proposer {
        return Err(HeaderError::Proposer);
    }
    if header.timestamp <= parent.timestamp {
        return Err(HeaderError::TimestampTooOld(header.timestamp));
    }
    // Proposals only: a decided block (sync) is final whatever the local clock says.
    if let Some(now) = now {
        if header.timestamp > now.saturating_add(max_drift) {
            return Err(HeaderError::TimestampTooNew(header.timestamp));
        }
    }
    Ok(())
}

/// Median of up to the last 11 timestamps (oldest first). Even counts take the upper median,
/// as Bitcoin does.
pub fn median_time_past(timestamps: &[u64]) -> u64 {
    let start = timestamps.len().saturating_sub(MTP_WINDOW);
    let mut window = timestamps[start..].to_vec();
    if window.is_empty() {
        return 0;
    }
    window.sort_unstable();
    window[window.len() / 2]
}
