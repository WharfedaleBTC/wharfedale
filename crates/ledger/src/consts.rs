//! Consensus constants. See `docs/src/constants.md`.

/// The only accepted transaction version (BIP68 always on).
pub const TX_VERSION: u32 = 2;

/// Largest explicit value per output and per-asset fee total (Elements `MAX_MONEY`).
pub const MAX_MONEY: u64 = 21_000_000 * 100_000_000;

/// Maximum transaction weight (weight units).
pub const MAX_TX_WEIGHT: usize = 400_000;

/// Maximum block weight (weight units), coinbase included.
pub const MAX_BLOCK_WEIGHT: usize = 4_000_000;

/// Coinbase `script_sig` length bounds (Bitcoin `CheckTransaction`).
pub const COINBASE_SCRIPT_SIG_MIN: usize = 2;
pub const COINBASE_SCRIPT_SIG_MAX: usize = 100;

/// nLockTime below this is a height, at or above it a unix time.
pub const LOCKTIME_THRESHOLD: u32 = 500_000_000;

/// BIP68 sequence fields.
pub const SEQUENCE_FINAL: u32 = 0xffff_ffff;
pub const SEQUENCE_LOCKTIME_DISABLE_FLAG: u32 = 1 << 31;
pub const SEQUENCE_LOCKTIME_TYPE_FLAG: u32 = 1 << 22;
pub const SEQUENCE_LOCKTIME_MASK: u32 = 0x0000_ffff;
pub const SEQUENCE_LOCKTIME_GRANULARITY: u32 = 9;

/// Taproot witness constants (BIP341, Elements).
pub const ANNEX_TAG: u8 = 0x50;
pub const TAPROOT_LEAF_MASK: u8 = 0xfe;
pub const TAPROOT_LEAF_SIMPLICITY: u8 = 0xbe;

/// Added to a Simplicity spend's witness size to give its cost budget (BIP342 `VALIDATION_WEIGHT_OFFSET`).
pub const VALIDATION_WEIGHT_OFFSET: i64 = 50;
