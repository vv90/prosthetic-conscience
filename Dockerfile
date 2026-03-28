FROM rust:1-slim AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
COPY static/ static/
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
