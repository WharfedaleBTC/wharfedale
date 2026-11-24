# Web app

`../wharfedale-web` (its own repository): the live network, explorer, demo wallet, faucet, asset
creator and a docs page, built on the TypeScript SDK. Screens: the network (home: an isometric
scene of the nodes in which each block pulses out from its proposer, plus the node list and
latest blocks), blocks list, block details, wallet (new, import, back up, send, receive), faucet,
asset creator, docs (the common flows in Rust and TypeScript with a language switcher, and links
into this book). A light grey canvas with dark green fields and a yellow-green accent, square
corners, Archivo for text and IBM Plex Mono for numbers and labels, scroll and headline
animations (off under reduced motion).

| Part | |
|---|---|
| App | Vite, React 19, React Router, TanStack Query, SVG; `@wharfedale/sdk` in the browser builds, blinds and signs |
| Server | Node 22, Hono: allowlisted JSON-RPC and Esplora proxy to one public node, `getnetworkinfo` of every configured node merged into a graph (with validators' consensus addresses), the faucet |
| Keys | Browser `localStorage`, unencrypted (demo). 12-word BIP39 phrase, key at `m/86'/1'/0'/0/0` |
| Asset names | Issued with on-chain metadata (`OP_RETURN "WFA1"`, contract hash = SHA-256(metadata)); read from `/asset/:id` |

Run with the Docker network (`docker/compose.yml` includes it):

```bash
make run
```

The app is on <http://localhost:8080>. `docker/web-config.json` points the server at `rpc0` (the
full node) for reads and submissions, lists all five nodes for the graph, reads the faucet
key from the `net` volume and links the docs page to `docsUrl` (this book on GitHub Pages). The
faucet keeps at least 20 spendable coins so it can pay many requests per block.
