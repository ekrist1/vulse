# syntax=docker/dockerfile:1

# Build the admin UI, then the server binary with the UI inside it.
FROM rust:1-bookworm AS build

ARG TRUNK_VERSION=0.21.14

# Node 22 builds the rich text editor bundle that Trunk's pre_build hook runs.
COPY --from=node:22-bookworm-slim /usr/local/bin/node /usr/local/bin/node
COPY --from=node:22-bookworm-slim /usr/local/lib/node_modules /usr/local/lib/node_modules
RUN ln -s /usr/local/lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm

RUN rustup target add wasm32-unknown-unknown \
    && curl -fsSL "https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}/trunk-x86_64-unknown-linux-gnu.tar.gz" \
    | tar -xz -C /usr/local/bin

WORKDIR /src
COPY crates/web/package.json crates/web/package-lock.json crates/web/
RUN npm --prefix crates/web ci

COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    trunk build --release --config crates/web/Trunk.toml \
    && cargo build --release --locked -p vulse-server --features embed-web \
    && mkdir -p /out \
    && cp target/release/vulse /out/vulse

# `docker build --target dist --output release .` writes the binary to ./release.
FROM scratch AS dist
COPY --from=build /out/ /

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /data vulse \
    && mkdir -p /data \
    && chown vulse:vulse /data

COPY --from=build /out/vulse /usr/local/bin/vulse

ENV BIND=0.0.0.0:8080 \
    VULSE_DB_PATH=/data/vulse.db \
    ASSET_DIR=/data/storage

USER vulse
WORKDIR /data
VOLUME /data
EXPOSE 8080
CMD ["vulse"]
