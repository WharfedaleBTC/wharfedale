//! Validation errors. One variant per rule; see `docs/src/ledger/validation.md`.

use std::fmt;

use elements::{AssetId, OutPoint, Txid};

/// Where a rangeproof lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProofLoc {
    Output(usize),
    IssuanceAmount(usize),
    IssuanceTokens(usize),
}

impl ProofLoc {
    /// The input index of an issuance location (the output index otherwise).
    pub fn input(self) -> usize {
        match self {
            Self::Output(i) | Self::IssuanceAmount(i) | Self::IssuanceTokens(i) => i,
        }
    }
}

impl fmt::Display for ProofLoc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Output(i) => write!(f, "output {i}"),
            Self::IssuanceAmount(i) => write!(f, "input {i} issuance amount"),
            Self::IssuanceTokens(i) => write!(f, "input {i} issuance tokens"),
        }
    }
}

/// Why a transaction is invalid.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TxError {
    // --- stateless ---
    #[error("transaction has no inputs")]
    NoInputs,
    #[error("transaction has no outputs")]
    NoOutputs,
    #[error("transaction weight {0} exceeds the limit")]
    TooHeavy(usize),
    #[error("transaction version {0} is not allowed")]
    Version(u32),
    #[error("input {0}: null prevout outside a coinbase")]
    NullPrevout(usize),
    #[error("input {0}: duplicate prevout")]
    DuplicateInput(usize),
    #[error("input {0}: peg-ins are not allowed")]
    Pegin(usize),
    #[error("input {0}: script_sig must be empty")]
    ScriptSigNotEmpty(usize),
    #[error("input {0}: peg-in witness must be empty")]
    PeginWitness(usize),
    #[error("input {0}: issuance rangeproof without a confidential issuance amount")]
    UnexpectedIssuanceProof(usize),
    #[error("output {0}: script kind is not allowed")]
    OutputScriptForbidden(usize),
    #[error("output {0}: null asset or value")]
    NullAssetOrValue(usize),
    #[error("output {0}: fee and OP_RETURN outputs must be explicit")]
    NotExplicit(usize),
    #[error("output {0}: zero value on a spendable or fee output")]
    ZeroValue(usize),
    #[error("output {0}: value out of range")]
    ValueOutOfRange(usize),
    #[error("sum of explicit output values exceeds MAX_MONEY")]
    OutputTotalOutOfRange,
    #[error("output {0}: proof attached to an explicit asset or value")]
    UnexpectedOutputProof(usize),
    #[error("fee total for asset {0} out of range")]
    FeeOutOfRange(AssetId),

    // --- context ---
    #[error("input {0}: spends missing or already spent output {1}")]
    MissingInput(usize, OutPoint),
    #[error("input {0}: spends an immature coinbase output")]
    ImmatureCoinbase(usize),
    #[error("lock_time not reached")]
    NotFinal,
    #[error("BIP68 relative lock not reached")]
    SequenceLocked,

    // --- amounts ---
    #[error("input {0}: spent output has an invalid explicit value")]
    SpentValueInvalid(usize),
    #[error("inputs and outputs don't balance")]
    BalanceMismatch,
    #[error("{0}: issuance amount must be explicit and positive, or committed")]
    IssuanceAmountInvalid(ProofLoc),
    #[error("input {0}: spent asset is not the reissuance token blinded with the revealed nonce")]
    ReissuanceTokenMismatch(usize),
    #[error("input {0}: a reissuance can't create reissuance tokens")]
    ReissuanceCreatesTokens(usize),
    #[error("{0}: missing rangeproof")]
    MissingRangeproof(ProofLoc),
    #[error("{0}: invalid rangeproof")]
    BadRangeproof(ProofLoc),
    #[error("{0}: rangeproof allows zero on a spendable output")]
    RangeproofMinValueZero(ProofLoc),
    #[error("output {0}: missing surjection proof")]
    MissingSurjectionProof(usize),
    #[error("output {0}: invalid surjection proof")]
    BadSurjectionProof(usize),

    // --- spends ---
    #[error("input {0}: spent output is not a taproot output")]
    SpentNotTaproot(usize),
    #[error("input {0}: empty witness")]
    WitnessEmpty(usize),
    #[error("input {0}: witness stack shape is not allowed")]
    WitnessShape(usize),
    #[error("input {0}: invalid signature encoding")]
    SignatureEncoding(usize),
    #[error("input {0}: sighash computation failed: {1}")]
    Sighash(usize, String),
    #[error("input {0}: signature verification failed")]
    BadSignature(usize),
    #[error("input {0}: invalid control block")]
    ControlBlock(usize),
    #[error("input {0}: taproot leaf version {1:#04x} is not allowed")]
    LeafVersionForbidden(usize, u8),
    #[error("input {0}: control block does not commit to the spent output key")]
    TaprootCommitment(usize),
    #[error("input {0}: Simplicity program rejected: {1}")]
    SimplicityRejected(usize, &'static str),
    /// The node could not evaluate the program (allocation failure, unimplemented jet).
    /// Not evidence that the transaction is invalid.
    #[error("input {0}: Simplicity evaluation fault: {1}")]
    SimplicityNodeFault(usize, &'static str),
}

/// Why a block is invalid.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BlockError {
    #[error("invalid coinbase: {0}")]
    Coinbase(&'static str),
    #[error("transaction {0} is a coinbase but not the first")]
    ExtraCoinbase(usize),
    #[error("block weight {0} exceeds the limit")]
    TooHeavy(usize),
    #[error("transaction {index} ({txid}): {error}")]
    Tx { index: usize, txid: Txid, error: TxError },
    #[error("output {0} already exists")]
    OutputExists(OutPoint),
    #[error("coinbase claims more than the fees for asset {0}")]
    CoinbaseOverclaim(AssetId),
}
