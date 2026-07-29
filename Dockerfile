FROM rust:1.82-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/
COPY config/ ./config/
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y \
    ca-certificates libssl3 ffmpeg ghostscript yt-dlp \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/ros-telegram-bot /usr/local/bin/
COPY config/i18n.json /app/config/i18n.json
COPY files/ /app/files/
WORKDIR /app
ENV RUST_LOG=info
EXPOSE 14380
ENTRYPOINT ["ros-telegram-bot"]
