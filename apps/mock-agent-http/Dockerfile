# syntax=docker/dockerfile:1

ARG TARGETPLATFORM
FROM --platform=$TARGETPLATFORM rust:1.95-bookworm AS builder
WORKDIR /workspace

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY apps/mock-agent-http ./apps/mock-agent-http

RUN cargo build --release -p mock-agent-http \
    && mkdir -p /out \
    && cp target/release/mock-agent-http /out/mock-agent-http

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /out/mock-agent-http /usr/local/bin/mock-agent-http
RUN chmod +x /usr/local/bin/mock-agent-http

EXPOSE 8080
ENV AGENTS_LISTEN_ADDR=0.0.0.0:8080
CMD ["/usr/local/bin/mock-agent-http"]
