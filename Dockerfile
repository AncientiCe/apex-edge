# Build stage
FROM rust:1-bookworm AS builder
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY apex-edge/ apex-edge/
COPY crates/ crates/
COPY tools/ tools/

RUN cargo build --release -p apex-edge

# Runtime stage
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -r -u 1000 apex
WORKDIR /app

COPY --from=builder /app/target/release/apex-edge /app/apex-edge

# Persistent data dir (mount a volume here for the DB)
RUN mkdir -p /data && chown apex:apex /data
ENV APEX_EDGE_DB=/data/apex_edge.db

USER apex
EXPOSE 3000

ENTRYPOINT ["/app/apex-edge"]
