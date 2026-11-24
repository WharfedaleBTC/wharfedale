# Wharfedale node + wallet image.
# Build context: the directory holding `wharfedale/`, `wharfedale-sdk-rs/` and `malachite/` (the fork), e.g.
#   docker build -f wharfedale/docker/Dockerfile -t wharfedale:dev .
# (docker/compose.yml does this for you.)

FROM rust:1-bookworm AS build
WORKDIR /src
COPY malachite /src/malachite
COPY wharfedale /src/wharfedale
COPY wharfedale-sdk-rs /src/wharfedale-sdk-rs
WORKDIR /src/wharfedale
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/wharfedale/target \
    cargo build --release --locked -p wharfedale-node -p wharfedale-wallet-cli \
 && cp target/release/wharfedale target/release/wharfedale-wallet /usr/local/bin/

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates bash \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/wharfedale /usr/local/bin/wharfedale-wallet /usr/local/bin/
COPY wharfedale/docker/scripts/ /usr/local/bin/
COPY wharfedale/examples/contracts/ /usr/local/share/wharfedale/contracts/
EXPOSE 27000 18443
ENV RUST_LOG=info
CMD ["wharfedale", "--help"]
