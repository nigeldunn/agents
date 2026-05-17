# syntax=docker/dockerfile:1

ARG TARGETPLATFORM
FROM --platform=$TARGETPLATFORM rust:1.95-bookworm AS builder
WORKDIR /workspace

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY apps ./apps

RUN cargo build --release -p agentcore-bridge \
    && mkdir -p /out \
    && cp target/release/agentcore-bridge /out/agentcore-bridge

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /out/agentcore-bridge /usr/local/bin/agentcore-bridge
RUN chmod +x /usr/local/bin/agentcore-bridge

EXPOSE 8080
ENV BRIDGE_LISTEN_ADDR=0.0.0.0:8080
CMD ["/usr/local/bin/agentcore-bridge"]
