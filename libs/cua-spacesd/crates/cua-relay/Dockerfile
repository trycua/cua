# cua-relay container image. Build from the repository's libs/ directory
# (the workspace path-depends on libs/cua and libs/cua-driver manifests):
#
#   docker build -f libs/cua-spacesd/crates/cua-relay/Dockerfile -t cua-relay libs
#   (add --build-arg CUA_RELAY_HOSTED=1 for the hosted relay's build)
#
# Terminate TLS in front of the relay (ingress / load balancer); it serves
# plain HTTP/1.1 and h2c on :8080.
#
# Reproducible: both base images are pinned by multi-arch index digest, the
# Rust toolchain matches libs/cua/rust-toolchain.toml, and the runtime stage
# runs no package manager (the CA bundle is assembled in a pinned stage).
# scripts/check-reproducible.sh builds twice and compares digests; see
# "Verify the relay image" in README.md. Bump the digests together with
# dependabot (.github/dependabot.yml); keep the rust tag equal to
# rust-toolchain.toml (CI checks that).
ARG RUST_IMAGE=rust:1.97.1-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97
ARG RUNTIME_IMAGE=debian:bookworm-slim@sha256:7ff6ba31527b9e336162fa061746c366c4b2032f4d2cff2078bea43f78efe3f8

FROM ${RUST_IMAGE} AS build
# Set to the commit time (git log -1 --format=%ct) so nothing embeds a clock.
ARG SOURCE_DATE_EPOCH=0
ENV SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH} \
    CARGO_INCREMENTAL=0 \
    RUSTFLAGS="--remap-path-prefix=/src=/build --remap-path-prefix=/usr/local/cargo=/cargo --remap-path-prefix=/usr/local/rustup=/rustup"
WORKDIR /src
COPY cua/Cargo.toml cua/Cargo.lock cua/
COPY cua/crates cua/crates
COPY cua/proto cua/proto
COPY cua-driver/rust cua-driver/rust
COPY cua-spacesd cua-spacesd
# The default image is the self-hosted relay: every account is Free, with no
# metering or Teams. The hosted relay (relay.cua.ai) adds the `hosted` feature
# (plan metering, Teams, account erasure, the hosted Spaces API's routes) from
# this same source: build it with
#   --build-arg CUA_RELAY_HOSTED=1
ARG CUA_RELAY_HOSTED=0
RUN cd cua-spacesd \
 && case "$CUA_RELAY_HOSTED" in \
      1|true|on) features="--features cua-relay/hosted" ;; \
      0|false|off|"") features="" ;; \
      *) echo "CUA_RELAY_HOSTED must be 0 or 1, got: $CUA_RELAY_HOSTED" >&2; exit 1 ;; \
    esac \
 && echo "cua-relay features: ${features:-default}" \
 && cargo build --release --locked -p cua-relay $features \
 && install -m 0755 target/release/cua-relay /usr/local/bin/cua-relay

# CA bundle, built from the pinned Rust image's ca-certificates package (no
# apt at build time, so it cannot drift).
# CUA-1948: the compromised eMudhra intermediates. Ubuntu's ca-certificates >= 20260816ubuntu0.<release>.1 drops the
# roots they chain to; Debian has not shipped that yet, so disable both roots explicitly (a no-op once they are gone).
FROM ${RUST_IMAGE} AS certs
RUN sed -i -E 's#^(mozilla/(emSign_ECC_Root_CA_-_G3|Comodo_AAA_Services_root)\.crt)$#!\1#' /etc/ca-certificates.conf \
 && update-ca-certificates --fresh

# Just the binary, for `docker buildx build --target binary --output type=local`
# (scripts/check-reproducible.sh hashes it).
FROM scratch AS binary
COPY --from=build /usr/local/bin/cua-relay /cua-relay

FROM ${RUNTIME_IMAGE}
COPY --from=certs /usr/share/ca-certificates /usr/share/ca-certificates
COPY --from=certs /etc/ca-certificates.conf /etc/ca-certificates.conf
COPY --from=certs /etc/ssl/certs /etc/ssl/certs
COPY --from=build /usr/local/bin/cua-relay /usr/local/bin/cua-relay
# The state directory (CUA_RELAY_STATE_FILE=/var/lib/cua-relay/machines.json):
# owned by the unprivileged user, so a new named volume mounted here is
# writable. A bind mount keeps the host directory's owner: chown it to 65532.
RUN install -d -o 65532 -g 65532 -m 0700 /var/lib/cua-relay
USER 65532:65532
EXPOSE 8080
ENV CUA_RELAY_LISTEN=0.0.0.0:8080
ENTRYPOINT ["/usr/local/bin/cua-relay"]
