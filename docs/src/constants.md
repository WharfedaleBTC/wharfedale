# Constants and open decisions

## Constants

| Constant | Value | Scope |
|---|---|---|
| Transaction version | 2 only | Consensus |
| `MAX_MONEY` (per asset, explicit values) | 21 000 000 × 10⁸ | Consensus (as Elements) |
| Simplicity budget offset | 50 WU | Consensus (must equal Elements) |
| Coinbase maturity | 1 block | Consensus |
| `MAX_TX_WEIGHT` | 400 000 WU (TBD) | Consensus |
| `MAX_BLOCK_WEIGHT` | 4 000 000 WU (TBD, size to the verification budget per round) | Consensus |
| `MAX_BLOCK_SIMPLICITY_COST` | TBD | Consensus |
| Timestamp drift | 10 s | Proposal validity |
| MTP window | 11 blocks | Consensus |
| `min_fee_per_kvb` | 10 units per 1 000 vbytes (default; `config.toml`) | Mempool policy |
| Wallet default fee (`DEFAULT_FEE`) | 100 units, flat | SDKs, CLI, web app |
| `MAX_POOL` | 300 MB vsize | Mempool policy |
| `MEMPOOL_EXPIRY` | 24 h | Mempool policy |
| Gossip batch | 50 ms / 64 KiB | Mempool policy |
| Undo retention N | 1 000 blocks | Storage |

The fee asset is the genesis-issued native asset. The deployments present it as Wrapped
Bitcoin (WBTC, 8 decimals: 1 unit = 1 sat); name, ticker and decimals are web-app config
(`nativeAsset`), not chain data. Its supply is fixed at genesis: 20 × 10¹² units = 200 000
WBTC to the faucet key (`FAUCET_OUTPUTS` × `FAUCET_OUTPUT_VALUE` in `testnet.rs`).

## Open decisions (settle before genesis)

- [ ] `state_root` in the header (recommended: yes, MuHash3072)
- [ ] Fee asset: a single genesis-issued asset (recommended) or any asset
- [ ] Genesis assets and holders of their reissuance tokens
- [ ] Taproot key-path spends allowed (recommended: yes)
- [ ] Validator key type: Ed25519 (recommended) or ECDSA
- [ ] Consensus upgrade mechanism: rules versioned by height, coordinated upgrades
- [ ] Validator key custody: file, HSM or remote signer
- [ ] Confidentiality policy: who receives blinding (view) keys
