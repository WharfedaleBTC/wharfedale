//! Application state and the `AppMsg` loop. Spec: `docs/src/consensus/app-loop.md`.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use elements::confidential::{Asset, Nonce, Value as ConfValue};
use elements::script::Builder;
use elements::{AssetId, BlockHash, LockTime, OutPoint, Script, Sequence, Transaction, TxIn, TxOut, TxOutWitness};
use eyre::{eyre, Result};
use malachitebft_app_channel::app::engine::host::{HeightParams, Next, SyncedValueOutcome};
use malachitebft_app_channel::app::streaming::{StreamContent, StreamId, StreamMessage};
use malachitebft_app_channel::app::types::core::utils::height::HeightRangeExt;
use malachitebft_app_channel::app::types::core::{
    ExtendedCommitCertificate, Round, Validity, VoteExtensionPolicy,
};
use malachitebft_app_channel::app::types::sync::RawDecidedValue;
use malachitebft_app_channel::app::types::{LocallyProposedValue, ProposedValue};
use malachitebft_app_channel::{AppMsg, Channels, NetworkMsg};
use prost::Message as _;
use tracing::{debug, error, info, warn};
use wharfedale_chain::{check_header, merkle, sha256d, Block, Header};
use wharfedale_consensus_ctx::codec::{decode_extended_commit_certificate, encode_extended_commit_certificate};
use wharfedale_consensus_ctx::{
    proto, Address, Ed25519Signer, Height, LinearTimeouts, ProposalData, ProposalFin, ProposalInit, ProposalPart,
    ValidatorSet, Value, WharfedaleContext,
};
use wharfedale_ledger::consts::MAX_BLOCK_WEIGHT;
use wharfedale_ledger::{connect_block, BlockDiff, BlockError, ChainCtx};
use wharfedale_rpc::NodeState;
use wharfedale_store::Store;

use crate::records;
use crate::streaming::{PartStreamsMap, ProposalParts};

/// Largest block slice carried by one proposal part.
pub const CHUNK_SIZE: usize = 64 * 1024;
/// Maximum seconds a proposal timestamp may be ahead of local time.
pub const MAX_TIME_DRIFT: u64 = 10;
/// Coinbase outputs wait this many blocks.
pub const COINBASE_MATURITY: u64 = 1;
/// Block weight kept free for the coinbase when filling from the mempool. The coinbase has at
/// most one output (fees are claimed in the fee asset only), so it stays well below this.
pub const COINBASE_WEIGHT_RESERVE: usize = 4_000;
/// Complete proposals for the next height held until it starts.
pub const MAX_FUTURE_PROPOSALS: usize = 16;

/// What a validator needs to propose: its consensus key and where its coinbase pays fees.
pub struct Proposer {
    pub signer: Ed25519Signer,
    pub fee_script: Script,
}

pub struct App {
    pub ctx: WharfedaleContext,
    /// Store, mempool and verification cache shared with RPC and gossip.
    pub node: NodeState,
    pub address: Address,
    validator_set: ValidatorSet,
    /// `None` on a full node: it never proposes.
    proposer: Option<Proposer>,
    block_time: Duration,
    streams: PartStreamsMap,
    height: Height,
    round: Round,
    /// Dry-run results of proposals, by block hash; applied on `Decided`.
    validated: HashMap<BlockHash, BlockDiff>,
    /// Complete, signed proposals for a later height, validated when that height starts.
    future: Vec<ProposalParts>,
    /// Publishes the committed tip height.
    tip: tokio::sync::watch::Sender<u64>,
}

impl App {
    pub fn new(
        node: NodeState,
        validator_set: ValidatorSet,
        address: Address,
        proposer: Option<Proposer>,
        block_time: Duration,
        tip: tokio::sync::watch::Sender<u64>,
    ) -> Result<Self> {
        tip.send_replace(node.store.tip()?.0);
        Ok(Self {
            ctx: WharfedaleContext,
            node,
            address,
            validator_set,
            proposer,
            block_time,
            streams: PartStreamsMap::new(),
            height: Height::new(1),
            round: Round::Nil,
            validated: HashMap::new(),
            future: Vec::new(),
            tip,
        })
    }

    fn height_params(&self) -> HeightParams<WharfedaleContext> {
        HeightParams::new(self.validator_set.clone(), LinearTimeouts::default(), Some(self.block_time))
            .with_vote_extension_policy(VoteExtensionPolicy::Disabled)
    }

    fn chain_ctx(&self, height: u64) -> Result<ChainCtx> {
        Ok(ChainCtx {
            height,
            median_time_past: self.node.store.mtp(height - 1)?,
            genesis_hash: self.node.genesis_hash,
            coinbase_maturity: COINBASE_MATURITY,
        })
    }

    fn store(&self) -> &Store {
        &self.node.store
    }

    /// Mempool template + coinbase claiming its fees, validated. Transactions that no longer
    /// validate are dropped from the template and the mempool.
    fn assemble(&self, h: u64, ctx: &ChainCtx, fee_script: &Script) -> Result<(Vec<Transaction>, BlockDiff)> {
        let max = MAX_BLOCK_WEIGHT - COINBASE_WEIGHT_RESERVE;
        let mut candidates: Vec<Transaction> =
            self.node.pool().template(max).iter().map(|t| (**t).clone()).collect();
        let fees = loop {
            let mut txs = vec![coinbase(h, 0, self.node.fee_asset, fee_script)];
            txs.extend(candidates.iter().cloned());
            match connect_block(&*self.node.store, &txs, ctx, &self.node.cache) {
                Ok(diff) => break diff.fees,
                Err(BlockError::Tx { index, txid, error }) => {
                    warn!(height = h, %txid, "dropping transaction from template: {error}");
                    candidates.remove(index - 1);
                    self.node.pool().remove(&txid);
                }
                Err(e) => return Err(e.into()),
            }
        };
        // Fees in other assets (only a non-standard proposer includes them) stay unclaimed.
        let claim = fees.get(&self.node.fee_asset).copied().unwrap_or(0);
        let mut txs = vec![coinbase(h, claim, self.node.fee_asset, fee_script)];
        txs.extend(candidates);
        let diff = connect_block(&*self.node.store, &txs, ctx, &self.node.cache)?;
        Ok((txs, diff))
    }

    fn expected_proposer(&self, height: Height, round: Round) -> Address {
        WharfedaleContext::proposer(&self.validator_set, height, round).address
    }

    // ---- building ----

    /// Builds, dry-runs and persists our block for (height, round). Re-returns a block already
    /// built for that slot, so a restarted node proposes the same block.
    fn propose(&mut self, height: Height, round: Round) -> Result<(Block, ProposalParts)> {
        let h = height.as_u64();
        let r = round.as_u32().ok_or_else(|| eyre!("propose in nil round"))?;
        let proposer = self.proposer.as_ref().ok_or_else(|| eyre!("a full node does not propose"))?;
        if let Some(hash) = self.store().built(h, r)? {
            if let (Some(record), Some(parts)) = (self.store().undecided_by_hash(h, &hash)?, self.store().parts(h, &hash)?) {
                let (_, block) = records::decode_proposal(&record)?;
                return Ok((block, records::decode_parts(&parts)?));
            }
        }

        let parent = self.store().header(h - 1)?.ok_or_else(|| eyre!("missing parent header {}", h - 1))?;
        let ctx = self.chain_ctx(h)?;
        let (txs, diff) = self.assemble(h, &ctx, &proposer.fee_script)?;
        let header = Header {
            version: Header::VERSION,
            height: h,
            prev_block_hash: parent.hash().to_byte_array(),
            timestamp: unix_now().max(parent.timestamp + 1),
            proposer: self.address.into_inner(),
            tx_merkle_root: merkle::tx_root(&txs),
            witness_merkle_root: merkle::witness_root(&txs),
        };
        let block = Block { header, txs };
        let hash = block.hash();
        let parts = self.sign_parts(&proposer.signer, &block, height, round);

        let pv = ProposedValue {
            height,
            round,
            valid_round: Round::Nil,
            proposer: self.address,
            value: Value::new(hash),
            validity: Validity::Valid,
        };
        self.store().put_undecided(h, r, &hash, &records::encode_proposal(&pv, &block))?;
        self.store().put_parts(h, &hash, &records::encode_parts(&parts)?)?;
        self.store().put_built(h, r, &hash)?;
        self.validated.insert(hash, diff);
        info!(height = h, round = r, %hash, "built block");
        Ok((block, parts))
    }

    fn sign_parts(&self, signer: &Ed25519Signer, block: &Block, height: Height, round: Round) -> ProposalParts {
        let bytes = block.encode();
        let chunks: Vec<Bytes> = bytes.chunks(CHUNK_SIZE).map(Bytes::copy_from_slice).collect();
        let signature = signer.sign(&parts_digest(height, round, Round::Nil, &chunks));
        let mut parts = vec![ProposalPart::Init(ProposalInit::new(height, round, Round::Nil, self.address))];
        parts.extend(chunks.into_iter().map(|c| ProposalPart::Data(ProposalData::new(c))));
        parts.push(ProposalPart::Fin(ProposalFin::new(signature)));
        ProposalParts { height, round, proposer: self.address, parts }
    }

    // ---- validation ----

    /// Proposer and signature check, then block reassembly and the structural check (the
    /// header commits to exactly these transactions). Failures mean a bad or forged stream,
    /// not an invalid proposal.
    fn open_parts(&self, parts: &ProposalParts) -> Result<Block> {
        if parts.proposer != self.expected_proposer(parts.height, parts.round) {
            return Err(eyre!("wrong proposer"));
        }
        let init = parts.init().ok_or_else(|| eyre!("missing Init"))?;
        if init.height != parts.height || init.round != parts.round || init.proposer != parts.proposer {
            return Err(eyre!("Init does not match the stream"));
        }
        let fin = parts.fin().ok_or_else(|| eyre!("missing Fin"))?;
        let chunks: Vec<Bytes> = parts.parts.iter().filter_map(|p| p.as_data()).map(|d| d.chunk.clone()).collect();
        let validator = self
            .validator_set
            .get_by_address(&parts.proposer)
            .ok_or_else(|| eyre!("proposer not in validator set"))?;
        let digest = parts_digest(parts.height, parts.round, init.pol_round, &chunks);
        if !Ed25519Signer::verify(&digest, &fin.signature, &validator.public_key) {
            return Err(eyre!("bad proposal signature"));
        }
        let bytes: Vec<u8> = chunks.concat();
        let block = Block::decode(&bytes).map_err(|e| eyre!("block decode: {e}"))?;
        if block.height() != parts.height.as_u64() {
            return Err(eyre!("block height does not match the proposal"));
        }
        if !block.roots_match() {
            return Err(eyre!("merkle roots do not match the transactions"));
        }
        Ok(block)
    }

    /// Full block validation on top of the committed tip (the caller checked the structure:
    /// `roots_match`). `now`: local time for a proposal, `None` for a decided (synced) block.
    /// Caches the diff on success.
    fn validate_block(&mut self, block: &Block, proposer: Address, now: Option<u64>) -> Result<()> {
        let h = block.height();
        let (tip, _) = self.store().tip()?;
        if h != tip + 1 {
            return Err(eyre!("block {h} does not extend tip {tip}"));
        }
        let parent = self.store().header(tip)?.ok_or_else(|| eyre!("missing tip header"))?;
        check_header(&block.header, &parent, &proposer.into_inner(), now, MAX_TIME_DRIFT)?;
        let diff = connect_block(&*self.node.store, &block.txs, &self.chain_ctx(h)?, &self.node.cache)?;
        self.validated.insert(block.hash(), diff);
        Ok(())
    }

    /// Validates complete parts for the current height and records them as undecided.
    fn accept_parts(&mut self, parts: ProposalParts) -> Result<Option<ProposedValue<WharfedaleContext>>> {
        let block = match self.open_parts(&parts) {
            Ok(block) => block,
            Err(e) => {
                warn!(height = %parts.height, round = %parts.round, "ignoring proposal: {e}");
                return Ok(None);
            }
        };
        let (h, r, hash) = (parts.height.as_u64(), round_u32(parts.round), block.hash());
        // A block is validated once per height: its outcome depends only on the parent.
        let known = self.store().undecided_by_hash(h, &hash)?.and_then(|rec| records::decode_proposal(&rec).ok());
        let validity = match known {
            Some((pv, _)) if pv.round == parts.round => return Ok(Some(pv)),
            Some((pv, _)) => pv.validity,
            None => match self.validate_block(&block, parts.proposer, Some(unix_now())) {
                Ok(()) => Validity::Valid,
                Err(e) => {
                    warn!(height = %parts.height, round = %parts.round, "invalid proposal: {e}");
                    Validity::Invalid
                }
            },
        };
        let init = parts.init().ok_or_else(|| eyre!("missing Init"))?;
        let pv = ProposedValue {
            height: parts.height,
            round: parts.round,
            valid_round: init.pol_round,
            proposer: parts.proposer,
            value: Value::new(block.hash()),
            validity,
        };
        self.store().put_undecided(h, r, &hash, &records::encode_proposal(&pv, &block))?;
        self.store().put_parts(h, &hash, &records::encode_parts(&parts)?)?;
        Ok(Some(pv))
    }

    // ---- commit ----

    /// Atomic commit of the decided block. Idempotent for an already committed height.
    fn commit(&mut self, cert: &ExtendedCommitCertificate<WharfedaleContext>) -> Result<()> {
        let h = cert.height.as_u64();
        let cert_bytes = encode_cert(cert)?;
        let (tip, _) = self.store().tip()?;
        if h <= tip {
            // Already committed (Decided, then Finalized with more signatures): keep the
            // stored block; a failed cert update is not worth stalling for.
            let stored = self.store().header(h)?.map(|hd| hd.hash());
            if stored != Some(cert.value_id.block_hash()) {
                return Err(eyre!("certificate for {h} does not match the committed block"));
            }
            if let Err(e) = self.store().put_cert(h, &cert_bytes) {
                warn!(height = h, "updating the stored certificate failed: {e}");
            }
            return Ok(());
        }
        let hash = cert.value_id.block_hash();
        let record = self
            .store()
            .undecided_by_hash(h, &hash)?
            .ok_or_else(|| eyre!("decided block {hash} at {h} is unknown"))?;
        let (_, block) = records::decode_proposal(&record)?;
        let diff = match self.validated.remove(&hash) {
            Some(diff) => diff,
            None => connect_block(&*self.node.store, &block.txs, &self.chain_ctx(h)?, &self.node.cache)?,
        };
        self.store().commit(&block, &cert_bytes, &diff)?;
        self.validated.clear();
        {
            let mut pool = self.node.pool();
            pool.remove_for_block(&block.txs);
            pool.expire(std::time::Instant::now());
        }
        self.tip.send_replace(h);
        info!(height = h, %hash, txs = block.txs.len(), "committed block");
        Ok(())
    }
}

/// Coinbase for `height`: claims `fee` of `fee_asset` to `fee_script`, or burns nothing with an
/// empty `OP_RETURN` when there is no fee. One output at most, whatever the block contains.
pub fn coinbase(height: u64, fee: u64, fee_asset: AssetId, fee_script: &Script) -> Transaction {
    let script_sig = Builder::new().push_int(height as i64).push_slice(b"wharfedale").into_script();
    let explicit = |asset: AssetId, value: u64, spk: Script| TxOut {
        asset: Asset::Explicit(asset),
        value: ConfValue::Explicit(value),
        nonce: Nonce::Null,
        script_pubkey: spk,
        witness: TxOutWitness::empty(),
    };
    let output = if fee > 0 {
        vec![explicit(fee_asset, fee, fee_script.clone())]
    } else {
        vec![explicit(AssetId::from_byte_array([0; 32]), 0, Script::new_op_return(&[]))]
    };
    Transaction {
        version: 2,
        lock_time: LockTime::ZERO,
        input: vec![TxIn { previous_output: OutPoint::null(), script_sig, sequence: Sequence::MAX, ..Default::default() }],
        output,
    }
}

/// What the proposer signs in `Fin`: height, round, POL round (from `Init`) and the chunks.
fn parts_digest(height: Height, round: Round, pol_round: Round, chunks: &[Bytes]) -> [u8; 32] {
    let mut buf = b"wharfedale/proposal/v2".to_vec();
    buf.extend_from_slice(&height.as_u64().to_le_bytes());
    buf.extend_from_slice(&round_u32(round).to_le_bytes());
    buf.extend_from_slice(&round_u32(pol_round).to_le_bytes());
    for c in chunks {
        buf.extend_from_slice(&(c.len() as u32).to_le_bytes());
        buf.extend_from_slice(c);
    }
    sha256d(&buf)
}

fn round_u32(r: Round) -> u32 {
    r.as_u32().unwrap_or(u32::MAX)
}

fn stream_messages(parts: &ProposalParts, height: Height, round: Round) -> Vec<StreamMessage<ProposalPart>> {
    let mut id = height.as_u64().to_be_bytes().to_vec();
    id.extend_from_slice(&round_u32(round).to_be_bytes());
    let stream_id = StreamId::new(id.into());
    let mut msgs: Vec<_> = parts
        .parts
        .iter()
        .enumerate()
        .map(|(seq, p)| StreamMessage::new(stream_id.clone(), seq as u64, StreamContent::Data(p.clone())))
        .collect();
    msgs.push(StreamMessage::new(stream_id, parts.parts.len() as u64, StreamContent::Fin));
    msgs
}

pub fn encode_cert(cert: &ExtendedCommitCertificate<WharfedaleContext>) -> Result<Vec<u8>> {
    Ok(encode_extended_commit_certificate(cert)?.encode_to_vec())
}

pub fn decode_cert(bytes: &[u8]) -> Result<ExtendedCommitCertificate<WharfedaleContext>> {
    Ok(decode_extended_commit_certificate(proto::ExtendedCommitCertificate::decode(bytes)?)?)
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The application loop. Returns when the consensus channel closes. A failure handling one
/// message (storage, network) is logged and the loop goes on: the dropped reply makes
/// consensus time out or retry, never stops the node.
pub async fn run(app: &mut App, channels: &mut Channels<WharfedaleContext>) -> Result<()> {
    while let Some(msg) = channels.consensus.recv().await {
        if let Err(e) = handle(app, channels, msg).await {
            error!("handling consensus message failed: {e:#}");
        }
    }
    Ok(())
}

async fn handle(
    app: &mut App,
    channels: &mut Channels<WharfedaleContext>,
    msg: AppMsg<WharfedaleContext>,
) -> Result<()> {
    {
        match msg {
            AppMsg::ConsensusReady { reply } => {
                let (tip, _) = app.node.store.tip()?;
                let start = Height::new(tip + 1);
                info!(%start, "consensus ready");
                let _ = reply.send((start, app.height_params()));
            }

            AppMsg::StartedRound { height, round, proposer, reply_value, .. } => {
                debug!(%height, %round, %proposer, "started round");
                app.height = height;
                app.round = round;
                for parts in std::mem::take(&mut app.future) {
                    if parts.height == height {
                        if let Err(e) = app.accept_parts(parts) {
                            warn!(%height, "buffered proposal: {e:#}");
                        }
                    } else if parts.height > height {
                        app.future.push(parts);
                    }
                }
                let values = app
                    .node
                    .store
                    .undecided_in_round(height.as_u64(), round_u32(round))?
                    .iter()
                    .filter_map(|r| records::decode_proposal(r).ok().map(|(pv, _)| pv))
                    .collect();
                let _ = reply_value.send(values);
            }

            AppMsg::GetValue { height, round, reply, .. } => {
                let (block, parts) = app.propose(height, round)?;
                let _ = reply.send(LocallyProposedValue::new(height, round, Value::new(block.hash())));
                for msg in stream_messages(&parts, height, round) {
                    channels.network.send(NetworkMsg::PublishProposalPart(msg)).await?;
                }
            }

            AppMsg::ReceivedProposalPart { from, published_by, part, reply } => {
                let value = match app.streams.insert(from, published_by, part) {
                    None => None,
                    Some(parts) if parts.height < app.height => None,
                    // Next height only, signed by its proposer, bounded.
                    Some(parts) if parts.height > app.height => {
                        if parts.height == app.height.increment() && app.future.len() < MAX_FUTURE_PROPOSALS {
                            match app.open_parts(&parts) {
                                Ok(_) => app.future.push(parts),
                                Err(e) => warn!(height = %parts.height, "ignoring future proposal: {e}"),
                            }
                        }
                        None
                    }
                    Some(parts) => app.accept_parts(parts)?,
                };
                let _ = reply.send(value);
            }

            AppMsg::RestreamProposal { height, round, value_id, .. } => {
                if let Some(raw) = app.node.store.parts(height.as_u64(), &value_id.block_hash())? {
                    let parts = records::decode_parts(&raw)?;
                    for msg in stream_messages(&parts, height, round) {
                        channels.network.send(NetworkMsg::PublishProposalPart(msg)).await?;
                    }
                }
            }

            AppMsg::Decided { certificate, extensions, reply } => {
                let cert = ExtendedCommitCertificate::from_commit_certificate_and_extensions(certificate, extensions);
                if let Err(e) = app.commit(&cert) {
                    error!(height = %cert.height, "commit on Decided failed: {e}");
                }
                let _ = reply.send(());
            }

            AppMsg::Finalized { certificate, extensions, evidence, reply } => {
                if !evidence.is_empty() {
                    warn!(height = %certificate.height, ?evidence, "misbehaviour evidence");
                }
                let height = certificate.height;
                let cert = ExtendedCommitCertificate::from_commit_certificate_and_extensions(certificate, extensions);
                let next = match app.commit(&cert) {
                    Ok(()) => Next::Start(height.increment(), app.height_params()),
                    Err(e) => {
                        error!(%height, "commit on Finalized failed, restarting height: {e}");
                        Next::Restart(height, app.height_params())
                    }
                };
                let _ = reply.send(next);
            }

            AppMsg::GetHistoryMinHeight { reply } => {
                let _ = reply.send(Height::new(app.node.store.min_body_height()?.max(1)));
            }

            AppMsg::GetDecidedValues { range, reply } => {
                let mut values = Vec::new();
                for height in range.iter_heights() {
                    let h = height.as_u64();
                    if let (Some(block), Some(cert)) = (app.node.store.block_bytes(h)?, app.node.store.cert(h)?) {
                        values.push(RawDecidedValue { value_bytes: block.into(), certificate: decode_cert(&cert)? });
                    }
                }
                let _ = reply.send(values);
            }

            AppMsg::ProcessSyncedValue { height, round, proposer, value_bytes, reply } => {
                let outcome = match Block::decode(&value_bytes) {
                    Err(_) => SyncedValueOutcome::PeerFault,
                    Ok(block) if block.height() != height.as_u64() => SyncedValueOutcome::PeerFault,
                    // The certificate commits to the header; a body it doesn't match is the peer's fault.
                    Ok(block) if !block.roots_match() => SyncedValueOutcome::PeerFault,
                    Ok(block) => {
                        let (tip, _) = app.node.store.tip()?;
                        if block.height() != tip + 1 {
                            SyncedValueOutcome::LocalTransientError
                        } else {
                            let validity = match app.validate_block(&block, proposer, None) {
                                Ok(()) => Validity::Valid,
                                Err(e) => {
                                    warn!(%height, "invalid synced block: {e}");
                                    Validity::Invalid
                                }
                            };
                            let pv = ProposedValue {
                                height,
                                round,
                                valid_round: Round::Nil,
                                proposer,
                                value: Value::new(block.hash()),
                                validity,
                            };
                            app.node.store.put_undecided(height.as_u64(), round_u32(round), &block.hash(), &records::encode_proposal(&pv, &block))?;
                            SyncedValueOutcome::Verdict(pv)
                        }
                    }
                };
                let _ = reply.send(outcome);
            }

            AppMsg::ExtendVote { reply, .. } => {
                let _ = reply.send(None);
            }

            AppMsg::VerifyVoteExtensions { extensions, reply, .. } => {
                let _ = reply.send(extensions.iter().map(|(a, _)| (*a, Ok(()))).collect());
            }
        }
    }
    Ok(())
}

