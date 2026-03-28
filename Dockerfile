FROM rust:1-slim AS builder
RUN apt-get update && apt-get install -y --no-install-recommends \
    gcc libc6-dev make perl && \
    rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
RUN cargo build --release --bin prosthetic-conscience --bin pc-worker

FROM debian:bookworm-slim AS gateway
RUN adduser --system --no-create-home gateway
COPY --from=builder /src/target/release/prosthetic-conscience /usr/local/bin/
USER gateway
EXPOSE 3000
ENTRYPOINT ["prosthetic-conscience", "--host", "0.0.0.0"]

FROM debian:bookworm-slim AS worker
RUN adduser --system --no-create-home worker
COPY --from=builder /src/target/release/pc-worker /usr/local/bin/
USER worker
ENTRYPOINT ["pc-worker"]
