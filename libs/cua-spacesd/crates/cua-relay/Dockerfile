# cua-relay container image. Build from the repository's libs/ directory
# (the workspace path-depends on libs/cua and libs/cua-driver manifests):
#
#   docker build -f libs/cua-spacesd/crates/cua-relay/Dockerfile -t cua-relay libs
#
# Terminate TLS in front of the relay (ingress / load balancer); it serves
# plain HTTP/1.1 and h2c on :8080.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY cua/Cargo.toml cua/Cargo.lock cua/
COPY cua/crates cua/crates
COPY cua/proto cua/proto
COPY cua-driver/rust cua-driver/rust
COPY cua-spacesd cua-spacesd
RUN cd cua-spacesd && cargo build --release --locked -p cua-relay \
 && install -m 0755 target/release/cua-relay /usr/local/bin/cua-relay

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/cua-relay /usr/local/bin/cua-relay
USER 65532:65532
EXPOSE 8080
ENV CUA_RELAY_LISTEN=0.0.0.0:8080
ENTRYPOINT ["/usr/local/bin/cua-relay"]
