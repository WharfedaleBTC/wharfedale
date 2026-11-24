# Docker

`docker/` holds one image and one Compose project: 4 validators, one full node (`rpc0`) and the
[web app](web.md). The `Makefile` in the repository root drives it; the `example` walkthrough
and the `e2e` test are scripts in the image, run inside `node0`.

| File | Purpose |
|---|---|
| `docker/Dockerfile` | Multi-stage: `rust:1-bookworm` builds `wharfedale` and `wharfedale-wallet` (`--release --locked`, cargo caches mounted); `debian:bookworm-slim` runtime with both binaries and the scripts |
| `docker/Dockerfile.dockerignore` | Excludes `target/`, `.git`, docs from the build context |
| `docker/compose.yml` | Services `init`, `node0`–`node3`, `rpc0`, `web`; volume `net`. No profiles |
| `Makefile` | `run`, `stop`, `clear`, `logs`, `ps`, `example`, `e2e`, `test` (`make` lists them) |
| `docker/scripts/example.sh` | Confidential faucet → alice → bob, issuance, reissuance, Esplora views, a p2pk contract |
| `docker/scripts/e2e.sh` | Assertions; exits non-zero on the first failure |
| `examples/contracts/*.simf` | Installed at `/usr/local/share/wharfedale/contracts/` in the image |

**Build context** is the directory containing `wharfedale/`, `wharfedale-sdk-rs/` (the wallet
CLI's SDK) and `malachite/` (the fork), plus `wharfedale-web/` and `wharfedale-sdk-ts/` for the
web app: `compose.yml` sets `context: ../..`. `--target binaries` exports just the two binaries
(CI uses it; see [CI and images](ci.md)).

## Services

| Service | Command | Notes |
|---|---|---|
| `init` | `wharfedale testnet --home /net --hosts node0,node1,node2,node3 --full-hosts rpc0 --chain-id wharfedale-docker` | Runs once: skipped if `/net/faucet.json` exists. Builds the image |
| `node0`…`node3` | `wharfedale start --home /net/node-<i>` | Validators. Start after `init` succeeds; P2P `27000`, RPC `18443` inside the network; RPC on host ports `18443`–`18446` |
| `rpc0` | `wharfedale start --home /net/full-0` | Full node (`role = "full"`); RPC on host port `18447` |
| `web` | `node dist/server/index.js` (image `wharfedale-web:dev`) | [Web app](web.md) on host port `8080`; config `docker/web-config.json` |

`--hosts` makes the generator write `listen = /ip4/0.0.0.0/tcp/27000`, peers as
`/dns4/node<i>/tcp/27000/p2p/<peer id>`, and `rpc.listen_addr = 0.0.0.0:18443`.

## Commands (from `wharfedale/`)

| Target | Does |
|---|---|
| `make run` | `docker compose -f docker/compose.yml up -d --build`: build the images, start everything |
| `make stop` | `down`: stop everything; the chain is kept |
| `make clear` | `down -v`: also delete the `net` volume, so the next `run` generates a new genesis |
| `make logs`, `make ps` | Follow the logs; list the services |
| `make example` | `exec -T node0 example.sh` on the running network |
| `make e2e` | `exec -T node0 e2e.sh` on the running network; non-zero exit on the first failed check |
| `make test` | `clear`, `run`, `e2e`: the checks on a fresh chain (wipes the current one) |

The faucet key is `/net/faucet.json` inside the volume:
`docker compose -f docker/compose.yml cp node0:/net/faucet.json .` copies it out.

## What `e2e.sh` checks

1. node0 answers, and an idle chain does not grow (no empty blocks).
2. faucet → alice (`wfc1` address) submitted to node0 is confirmed as seen by node3.
3. alice → bob (confidential input and outputs) submitted to node2 is confirmed as seen by node1.
4. Unblinded balances: alice 3 999 900, bob 1 000 000 (default fee 100).
5. The payment shows value commitments on chain (Esplora `/tx`).
6. alice issues 1 000 units + 1 reissuance token; reissues 500 via node2.
7. Esplora: the asset record shows 1 500 issued; `outspends` shows the spent faucet output.
8. An overspend is refused.
9. faucet locks 300 000 to bob's p2pk contract; alice's signature can't spend it; bob's spends
   200 000 to alice (change 99 000 stays in the contract).
10. Timelock contract (`AFTER` = tip + 3): lock time `AFTER − 1` refused; `AFTER` spends.
11. A transaction submitted to the full node `rpc0` is confirmed by the validators.
12. All four validators and the full node report the same block hash at the current height.
