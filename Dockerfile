FROM rust:1.82-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/
COPY config/ ./config/
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y \
    ca-certificates libssl3 ffmpeg ghostscript yt-dlp curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/rostam-dev /usr/local/bin/
COPY config/i18n.json /app/config/i18n.json
COPY files/ /app/files/
WORKDIR /app
ENV RUST_LOG=info
ENV HEALTH_PORT=14380
EXPOSE 14380
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD curl -f http://127.0.0.1:${HEALTH_PORT}/health || exit 1
ENTRYPOINT ["rostam-dev"]
