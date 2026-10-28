# SwarmPanel (Rust): builds the WASM browser module, then the server (which
# embeds it, the CSS and images), into one small runtime image.
FROM rust:1-bookworm AS build
RUN rustup target add wasm32-unknown-unknown \
 && cargo install wasm-bindgen-cli --version 0.2.100 --locked
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY server server
COPY web web
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/src/target \
    cargo build --release -p swarmpanel-web --target wasm32-unknown-unknown \
 && wasm-bindgen --target web --no-typescript --out-dir web/pkg target/wasm32-unknown-unknown/release/swarmpanel_web.wasm \
 && cargo build --release -p swarmpanel \
 && cp target/release/swarmpanel /swarmpanel

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates tzdata && rm -rf /var/lib/apt/lists/*
COPY --from=build /swarmpanel /usr/local/bin/swarmpanel
ENV PANEL_ROOT=/app
WORKDIR /app
CMD ["swarmpanel"]
