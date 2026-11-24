//! `wharfedale`: the node. Wires Malachite, ledger, store (and, from M3, mempool and RPC).
//!
//! Spec: `docs/src/architecture.md`, `docs/src/consensus/app-loop.md`.
#![forbid(unsafe_code)]

pub mod app;
pub mod config;
pub mod gossip;
pub mod keys;
pub mod node;
pub mod records;
pub mod streaming;
pub mod testnet;
