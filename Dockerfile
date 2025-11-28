FROM rust:1.91 AS builder

WORKDIR /app

# Install build tooling needed for native dependencies
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential pkg-config cmake clang ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release --locked

FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

COPY --from=builder /app/target/release/notion2md-server /app/notion2md-server

EXPOSE 3000
ENV RUST_LOG=info

USER nonroot
ENTRYPOINT ["/app/notion2md-server"]
