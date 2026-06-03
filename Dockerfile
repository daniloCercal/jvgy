# syntax=docker/dockerfile:1

# ---- Builder ----
FROM rust:1.96-slim-bookworm AS builder
WORKDIR /app
# Pre-build dependencies for layer caching (dummy lib+bin sources).
COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
    && echo "fn main() {}" > src/main.rs \
    && touch src/lib.rs \
    && cargo build --release --locked \
    && rm -rf src
# Build the real sources.
COPY . .
RUN touch src/main.rs src/lib.rs && cargo build --release --locked

# ---- Runtime ----
FROM debian:bookworm-slim
# ffmpeg + streamlink are the only runtime media deps; ca-certificates is needed
# by streamlink (Python/TLS) to reach Twitch. The bot itself uses bundled rustls
# roots and needs no system certs.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ffmpeg streamlink ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/twitch-discord-summarizer /usr/local/bin/summarizer
RUN useradd -r -u 10001 appuser
USER appuser
# Config is via env / mounted .env. SIGTERM triggers graceful drain.
ENTRYPOINT ["/usr/local/bin/summarizer"]
