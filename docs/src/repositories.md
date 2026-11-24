# Repositories and dependencies

| Repository | Contents |
|---|---|
| `wharfedale` ([github.com/WharfedaleBTC/wharfedale](https://github.com/WharfedaleBTC/wharfedale)) | This workspace |
| `wharfedale-sdk-rs` | Rust client SDK; path dependency of `wharfedale-wallet` ([SDKs](sdk.md)) |
| `wharfedale-sdk-ts` | TypeScript client SDK ([SDKs](sdk.md)) |
| `wharfedale-web` | Web app: explorer, network graph, demo wallet, faucet, asset creator ([Web app](web.md)) |
| `malachite` ([github.com/WharfedaleBTC/malachite](https://github.com/WharfedaleBTC/malachite), fork of `circlefin/malachite`) | `main` = upstream `a453b32` (v0.8.2); the patch is branch `wharfedale/mempool-channel` ([patch](consensus/mempool-channel.md)). Docker builds copy this checkout |

Local layout:

```text
Liquid2/
├── wharfedale/          # this repo
├── wharfedale-sdk-rs/   # Rust SDK; the wallet CLI depends on it by path
├── wharfedale-sdk-ts/   # TypeScript SDK
├── wharfedale-web/      # web app (uses the TypeScript SDK)
└── malachite/           # fork checkout; workspace path deps point here
```

## Pinned versions

| Crate | Version | Note |
|---|---|---|
| `elements` | 0.27.0 | Types, encoding, sighash, issuance, blinding, PSET |
| `secp256k1-zkp` | 0.11 (via `elements::secp256k1_zkp`) | Never a direct dependency: one copy only |
| `simplicity-lang` | 0.9.0, feature `elements` | Program types, CMR, satisfier |
| `simplicity-sys` | 0.8.0, feature `test-utils` | Builds the C library (`test-utils` compiles the decoder and evaluator); exec FFI declared in our sys crate |
| `simplicityhl` | 0.9.0 | Contract compiler: wallet and tests only, never consensus |
| `arc-malachitebft-*` | fork rev | Path deps in development; `git` + `rev` in releases |
| `redb` | 2.6 | Storage |

Malachite crates used: `app-channel`, `app`, `codec`, `config`, `core-consensus`,
`core-types`, `proto`, `signing`, `signing-ed25519`, `sync`. `wharfedale-consensus-ctx` and
`node::streaming` adapt code from Malachite's `test` and `test/streaming` crates (Apache-2.0;
each file says so).

## Rules

- All `arc-malachitebft-*` crates come from one checkout or one rev. Mixed revs give mismatched trait types.
- If any dependency pulls Malachite crates from crates.io, add `[patch.crates-io]` redirecting them to the fork.
- Releases replace the `path` deps with `git = "<fork url>", rev = "<sha>"`.
- Upgrading Malachite: rebase the fork branch on the new upstream, run its tests, bump the rev here in one commit.
- Pin the Simplicity C library commit to the one Elements consensus uses.
