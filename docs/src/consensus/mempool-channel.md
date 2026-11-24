# Malachite patch: Mempool channel

Branch `wharfedale/mempool-channel` of the fork (on upstream `a453b32`) adds a fifth gossip
channel carrying opaque application bytes. It is off unless `p2p.enable_mempool = true`, so
upstream behaviour is unchanged. Every change is marked `WHARFEDALE:` in the source.

```text
app                              app-channel / engine                     network (libp2p)
NetworkMsg::PublishTransactions(B) → network::Msg::PublishMempoolMsg(B) → publish(Channel::Mempool, B)

channels.mempool.recv()          ← mpsc (1024, try_send) ← Msg::NewEvent  ← Event::MempoolMessage
  MempoolInbound { from, published_by, data }   (set once by SetMempoolSink; bypasses OutputPort)
```

## Changes

| Crate | File | Change |
|---|---|---|
| `config` | `src/lib.rs` | `ChannelNames.mempool` (default `/mempool`, serde default, in `validate()`); `PubSubMaxSizePerTopic.mempool`; `P2pConfig.enable_mempool` (default `false`) |
| `network` | `src/channel.rs` | `Channel::Mempool`; `Channel::mempool()`; included in `all()`, topic lookups and the gossipsub subscription whitelist |
| `network` | `src/lib.rs` | `Config.enable_mempool`; `PubSubMaxSizePerTopic.mempool`; `Event::MempoolMessage(PeerId, Option<PeerId>, Bytes)`; subscribe when enabled; map inbound gossipsub and broadcast frames |
| `network` | `src/pubsub.rs`, `src/behaviour.rs` | Per-topic size limit for Mempool |
| `app` | `src/spawn.rs` | Pass channel name, size limit and `enable_mempool` into the network config |
| `engine` | `src/network.rs` | `MempoolInbound`; `Msg::PublishMempoolMsg(Bytes)`; `Msg::SetMempoolSink(Sender)`; `State::Running.mempool_sink`; inbound → `try_send` (dropped when full) |
| `app-channel` | `src/msgs.rs`, `src/lib.rs` | `NetworkMsg::PublishTransactions(Bytes)`; `Channels.mempool: Receiver<MempoolInbound>`; re-export `MempoolInbound` |
| `app-channel` | `src/builder.rs` | Create the channel; send `SetMempoolSink` to the network actor (default and custom network paths) |
| tests | `network/src/channel.rs`, `config` tests, `network/test` | Whitelist test expects the Mempool topic; struct literals gain the new fields |

Size: ~200 lines of code, ~20 of test fixtures.

## Properties

- Inbound application bytes never reach the consensus or sync actors; `NetworkEvent` is unchanged.
- `try_send` into a 1024-slot channel: a slow app loses transaction messages, never delays votes.
- Payloads are raw bytes: no codec or `Context` change.
- Gossipsub relays before the app validates (`ValidationMode::Strict`, no manual validation);
  manual validation is a global switch and stays off. Gossiped transactions are not re-published
  by the app.
- Message ids hash the whole gossip message, so the same transaction published twice has two
  ids. The mempool deduplicates by wtxid.

## Payload (`wharfedale_mempool::wire`)

`version (u8 = 1) ‖ count (u32 LE) ‖ (len (u32 LE) ‖ Elements tx)*`. The node flushes a batch
every 50 ms or at 64 KiB (`node::gossip`).

## Verification

- `cargo check --workspace --all-targets` in the fork: clean.
- Unit tests of `config`, `network`, `app`, `app-channel`, `engine`: pass. `network/test`
  (discovery) suite: pass (`discovery_disabled` is timing-sensitive and failed once in a parallel
  run; it passes alone and in a full rerun).
- End to end through Wharfedale: `crates/node/tests/transfer.rs` and the Docker e2e test.
