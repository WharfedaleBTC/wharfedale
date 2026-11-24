# CI and images

Each repository's `.github/workflows/ci.yml` tests it and publishes it on GitHub. Every action
is pinned to a commit (with its version in a comment), chosen at least two weeks old.

| Repository | On pull requests | On `master` | On a `vX.Y.Z` tag |
|---|---|---|---|
| `wharfedale` (node) | clippy `-D warnings`, `cargo test --workspace` | + image `ghcr.io/wharfedalebtc/wharfedale:latest`, `:sha-<short>`; binaries as workflow artifacts | + image `:X.Y.Z`, `:X.Y`; a GitHub Release with the binaries |
| `wharfedale-web` | build (type-checks app and server) | + image `ghcr.io/wharfedalebtc/wharfedale-web:latest`, `:sha-<short>`; the built app as a workflow artifact | + image `:X.Y.Z`, `:X.Y`; a GitHub Release with the built app |

Images are multi-arch (linux/amd64, linux/arm64). The node's are built natively on each
architecture (`ubuntu-latest`, `ubuntu-24.04-arm`), pushed by digest and joined under the tags
by `docker buildx imagetools create`; the same build exports the binaries
(`--target binaries`). The web app's image is one QEMU build.

## Sibling checkouts

CI lays the repositories out as locally, so path dependencies resolve:

| Workflow | Checks out | Ref (workflow `env`) |
|---|---|---|
| node | `WharfedaleBTC/malachite` → `malachite/` | `MALACHITE_REF` = `wharfedale/mempool-channel` |
| node | `WharfedaleBTC/wharfedale-sdk-rs` → `wharfedale-sdk-rs/` | `SDK_RS_REF` = `master` |
| web | `WharfedaleBTC/wharfedale-sdk-ts` → `wharfedale-sdk-ts/` | `SDK_TS_REF` = `master` |

A change that spans repositories lands in the dependency first (fork, then SDK), then here.

## Repository settings

- `wharfedale-sdk-rs` is private: add a secret `SDK_RS_TOKEN` to `wharfedale` (a fine-grained
  token with read access to that repository's contents). Without it, and while it stays
  private, the node workflow cannot check it out. Once it is public, no secret is needed.
- The first push creates the packages on GHCR, private by default. To pull without a login
  (as `demo-prod` does), set each package's visibility to public: the organization's
  Packages → the package → Package settings.
- Releases: push a tag, `git tag v0.1.0 && git push origin v0.1.0`.

## `demo-prod`

`../demo-prod` (its own folder) runs a small network from the published images: two
validators, one full node and the web app, with `make run | stop | clear | update`. See its
README.
