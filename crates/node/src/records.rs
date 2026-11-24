//! Encodings of the node's in-flight records stored in `wharfedale-store`.
//!
//! - proposal: `proposer (20) ‖ round (u32 LE, MAX = nil) ‖ valid_round (u32 LE, MAX = nil) ‖
//!   validity (u8) ‖ block bytes`
//! - parts: `height (u64 LE) ‖ round (u32 LE) ‖ proposer (20) ‖ count (u32 LE) ‖
//!   (len (u32 LE) ‖ protobuf ProposalPart)*`

use bytes::Bytes;
use eyre::{eyre, Result};
use malachitebft_app_channel::app::types::codec::Codec;
use malachitebft_app_channel::app::types::core::{Round, Validity};
use malachitebft_app_channel::app::types::ProposedValue;
use wharfedale_chain::Block;
use wharfedale_consensus_ctx::codec::ProtobufCodec;
use wharfedale_consensus_ctx::{Address, Height, ProposalPart, Value, WharfedaleContext};

use crate::streaming::ProposalParts;

fn round_to_u32(r: Round) -> u32 {
    r.as_u32().unwrap_or(u32::MAX)
}

fn round_from_u32(r: u32) -> Round {
    if r == u32::MAX { Round::Nil } else { Round::new(r) }
}

pub fn encode_proposal(pv: &ProposedValue<WharfedaleContext>, block: &Block) -> Vec<u8> {
    let mut out = pv.proposer.into_inner().to_vec();
    out.extend_from_slice(&round_to_u32(pv.round).to_le_bytes());
    out.extend_from_slice(&round_to_u32(pv.valid_round).to_le_bytes());
    out.push(u8::from(pv.validity.is_valid()));
    out.extend_from_slice(&block.encode());
    out
}

pub fn decode_proposal(bytes: &[u8]) -> Result<(ProposedValue<WharfedaleContext>, Block)> {
    if bytes.len() < 29 {
        return Err(eyre!("proposal record too short"));
    }
    let proposer = Address::new(bytes[..20].try_into()?);
    let round = round_from_u32(u32::from_le_bytes(bytes[20..24].try_into()?));
    let valid_round = round_from_u32(u32::from_le_bytes(bytes[24..28].try_into()?));
    let validity = if bytes[28] == 1 { Validity::Valid } else { Validity::Invalid };
    let block = Block::decode(&bytes[29..]).map_err(|e| eyre!("proposal block: {e}"))?;
    let pv = ProposedValue {
        height: Height::new(block.height()),
        round,
        valid_round,
        proposer,
        value: Value::new(block.hash()),
        validity,
    };
    Ok((pv, block))
}

pub fn encode_parts(parts: &ProposalParts) -> Result<Vec<u8>> {
    let mut out = parts.height.as_u64().to_le_bytes().to_vec();
    out.extend_from_slice(&round_to_u32(parts.round).to_le_bytes());
    out.extend_from_slice(&parts.proposer.into_inner());
    out.extend_from_slice(&(parts.parts.len() as u32).to_le_bytes());
    for part in &parts.parts {
        let bytes = ProtobufCodec.encode(part)?;
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&bytes);
    }
    Ok(out)
}

pub fn decode_parts(bytes: &[u8]) -> Result<ProposalParts> {
    let take = |pos: &mut usize, n: usize| -> Result<&[u8]> {
        let s = bytes.get(*pos..*pos + n).ok_or_else(|| eyre!("parts record truncated"))?;
        *pos += n;
        Ok(s)
    };
    let mut pos = 0;
    let height = Height::new(u64::from_le_bytes(take(&mut pos, 8)?.try_into()?));
    let round = round_from_u32(u32::from_le_bytes(take(&mut pos, 4)?.try_into()?));
    let proposer = Address::new(take(&mut pos, 20)?.try_into()?);
    let count = u32::from_le_bytes(take(&mut pos, 4)?.try_into()?);
    let mut parts = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let len = u32::from_le_bytes(take(&mut pos, 4)?.try_into()?) as usize;
        let raw = Bytes::copy_from_slice(take(&mut pos, len)?);
        parts.push(Codec::<ProposalPart>::decode(&ProtobufCodec, raw)?);
    }
    Ok(ProposalParts { height, round, proposer, parts })
}
