// Adapted from Malachite `code/crates/test/streaming` (Apache-2.0), commit a453b32.
use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap, HashSet, VecDeque};

use malachitebft_app_channel::app::consensus::PeerId;
use malachitebft_app_channel::app::streaming::{Sequence, StreamId, StreamMessage};
use malachitebft_app_channel::app::types::core::Round;
use wharfedale_consensus_ctx::{Address, Height, ProposalFin, ProposalInit, ProposalPart};

struct MinSeq<T>(StreamMessage<T>);

impl<T> PartialEq for MinSeq<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0.sequence == other.0.sequence
    }
}

impl<T> Eq for MinSeq<T> {}

impl<T> Ord for MinSeq<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        other.0.sequence.cmp(&self.0.sequence)
    }
}

impl<T> PartialOrd for MinSeq<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

struct MinHeap<T>(BinaryHeap<MinSeq<T>>);

impl<T> Default for MinHeap<T> {
    fn default() -> Self {
        Self(BinaryHeap::new())
    }
}

impl<T> MinHeap<T> {
    fn push(&mut self, msg: StreamMessage<T>) {
        self.0.push(MinSeq(msg));
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn drain(&mut self) -> Vec<T> {
        let mut vec = Vec::with_capacity(self.0.len());
        while let Some(MinSeq(msg)) = self.0.pop() {
            if let Some(data) = msg.content.into_data() {
                vec.push(data);
            }
        }
        vec
    }
}

#[derive(Default)]
struct StreamState {
    buffer: MinHeap<ProposalPart>,
    init_info: Option<ProposalInit>,
    seen_sequences: HashSet<Sequence>,
    total_messages: usize,
    fin_received: bool,
}

impl StreamState {
    fn is_done(&self) -> bool {
        self.init_info.is_some() && self.fin_received && self.buffer.len() == self.total_messages
    }

    fn insert(&mut self, msg: StreamMessage<ProposalPart>) -> Option<ProposalParts> {
        if msg.is_first() {
            self.init_info = msg.content.as_data().and_then(|p| p.as_init()).cloned();
        }

        if msg.is_fin() {
            self.fin_received = true;
            self.total_messages = msg.sequence as usize + 1;
        }

        self.buffer.push(msg);

        if self.is_done() {
            let init_info = self.init_info.take()?;

            Some(ProposalParts {
                height: init_info.height,
                round: init_info.round,
                proposer: init_info.proposer,
                parts: self.buffer.drain(),
            })
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProposalParts {
    pub height: Height,
    pub round: Round,
    pub proposer: Address,
    pub parts: Vec<ProposalPart>,
}

impl ProposalParts {
    pub fn init(&self) -> Option<&ProposalInit> {
        self.parts.iter().find_map(|p| p.as_init())
    }

    pub fn fin(&self) -> Option<&ProposalFin> {
        self.parts.iter().find_map(|p| p.as_fin())
    }
}

/// Streams assembled at once; the oldest is dropped beyond this (any peer can open streams).
pub const MAX_STREAMS: usize = 64;
/// Highest sequence number accepted: a maximal block (4 MB) is ~67 messages of 64 KiB.
pub const MAX_SEQUENCE: u64 = 80;

#[derive(Default)]
pub struct PartStreamsMap {
    /// Keyed by the publisher of the stream: the parts of one proposal can
    /// arrive through different peers and must land in the same entry.
    streams: BTreeMap<(PeerId, StreamId), StreamState>,
    /// Stream keys, oldest first (may hold keys of finished streams).
    order: VecDeque<(PeerId, StreamId)>,
}

impl PartStreamsMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a proposal part.
    ///
    /// `delivered_by` is the peer the part arrived from. `published_by` is the
    /// publisher declared in the message, used to group parts of one proposal;
    /// it falls back to `delivered_by` for transports that carry no publisher.
    pub fn insert(
        &mut self,
        delivered_by: PeerId,
        published_by: Option<PeerId>,
        msg: StreamMessage<ProposalPart>,
    ) -> Option<ProposalParts> {
        let stream_id = msg.stream_id.clone();
        let publisher = published_by.unwrap_or(delivered_by);
        if msg.sequence > MAX_SEQUENCE {
            return None;
        }
        let key = (publisher, stream_id.clone());
        if !self.streams.contains_key(&key) {
            while self.streams.len() >= MAX_STREAMS {
                let Some(old) = self.order.pop_front() else { break };
                self.streams.remove(&old);
            }
            self.order.retain(|k| self.streams.contains_key(k));
            self.order.push_back(key.clone());
        }

        let state = self.streams.entry(key).or_default();

        if !state.seen_sequences.insert(msg.sequence) {
            // We have already seen a message with this sequence number.
            return None;
        }

        let result = state.insert(msg);

        if state.is_done() {
            self.streams.remove(&(publisher, stream_id));
        }

        result
    }
}

