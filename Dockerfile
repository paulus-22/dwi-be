# syntax=docker/dockerfile:1
# Two stages: compile with the full toolchain, ship a ~30 MB image.
# TLS is rustls, so the runtime image needs no OpenSSL.

FROM rust:1.99-slim-bookworm AS build
WORKDIR /src
# Cache dependencies: build once with stub sources, then with the real ones.
COPY Cargo.toml Cargo.lock rustfmt.toml ./
COPY crates/domain/Cargo.toml crates/domain/
COPY crates/api/Cargo.toml crates/api/
RUN mkdir -p crates/domain/src crates/api/src \
 && echo "" > crates/domain/src/lib.rs \
 && echo "fn main() {}" > crates/api/src/main.rs && echo "" > crates/api/src/lib.rs \
 && cargo build --release --locked -p dwi-api || true
COPY crates crates
COPY migrations migrations
RUN touch crates/domain/src/lib.rs crates/api/src/lib.rs crates/api/src/main.rs \
 && cargo build --release --locked -p dwi-api

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* && useradd --system --uid 10001 dwi
COPY --from=build /src/target/release/dwi-api /usr/local/bin/dwi-api
USER dwi
ENV BIND_ADDR=0.0.0.0:8080
EXPOSE 8080
# Runs migrations on start, then serves.
CMD ["dwi-api"]
