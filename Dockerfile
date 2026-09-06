FROM rust:1.88-slim AS builder
WORKDIR /build

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
RUN cargo build --release --locked

FROM debian:13.6-slim
WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl3 ca-certificates curl tini bash \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/cronmanager /app/cronmanager
# Ship the demo self-contained: default config, sample DSLs, and
# sample scripts. Operators bind-mount over any of these to override.
COPY cronmanager.yaml /app/cronmanager.yaml
COPY DSL /app/DSL
COPY scripts /app/scripts
COPY migrations /app/migrations

RUN chmod +x /app/scripts/samples/*.sh 2>/dev/null || true

EXPOSE 8080
RUN useradd -m -u 1000 cronmanager && chown -R cronmanager:cronmanager /app
USER cronmanager

ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["/app/cronmanager"]

HEALTHCHECK --interval=30s --timeout=3s --start-period=15s --retries=3 \
    CMD curl -fsS http://localhost:8080/health || exit 1
