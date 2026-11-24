//! Consensus value: a reference to a block (its hash). The block itself travels as proposal
//! parts (ADR-003 `ProposalAndParts`).

use core::fmt;

use malachitebft_proto::{Error as ProtoError, Protobuf};
use serde::{Deserialize, Serialize};

use crate::proto;

/// Block hash, as voted on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ValueId(pub [u8; 32]);

impl ValueId {
    pub fn block_hash(&self) -> elements::BlockHash {
        elements::BlockHash::from_byte_array(self.0)
    }
}

impl From<elements::BlockHash> for ValueId {
    fn from(hash: elements::BlockHash) -> Self {
        Self(hash.to_byte_array())
    }
}

impl fmt::Display for ValueId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.block_hash())
    }
}

/// The value consensus decides on: `BlockRef` = block hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Value(pub ValueId);

impl Value {
    pub fn new(hash: elements::BlockHash) -> Self {
        Self(hash.into())
    }

    pub fn id(&self) -> ValueId {
        self.0
    }
}

impl malachitebft_core_types::Value for Value {
    type Id = ValueId;

    fn id(&self) -> ValueId {
        self.0
    }
}

fn hash_from_bytes(bytes: Option<bytes::Bytes>) -> Result<[u8; 32], ProtoError> {
    let bytes = bytes.ok_or_else(|| ProtoError::Other("missing value".into()))?;
    <[u8; 32]>::try_from(bytes.as_ref())
        .map_err(|_| ProtoError::Other(format!("value must be 32 bytes, got {}", bytes.len())))
}

impl Protobuf for ValueId {
    type Proto = proto::ValueId;

    fn from_proto(proto: Self::Proto) -> Result<Self, ProtoError> {
        Ok(Self(hash_from_bytes(proto.value)?))
    }

    fn to_proto(&self) -> Result<Self::Proto, ProtoError> {
        Ok(proto::ValueId { value: Some(self.0.to_vec().into()) })
    }
}

impl Protobuf for Value {
    type Proto = proto::Value;

    fn from_proto(proto: Self::Proto) -> Result<Self, ProtoError> {
        Ok(Self(ValueId(hash_from_bytes(proto.value)?)))
    }

    fn to_proto(&self) -> Result<Self::Proto, ProtoError> {
        Ok(proto::Value { value: Some(self.0 .0.to_vec().into()) })
    }
}
