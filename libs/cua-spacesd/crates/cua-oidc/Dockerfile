# cua-oidc container image (the Cua OIDC issuer). Build from the
# repository's libs/ directory (the workspace path-depends on libs/cua and
# libs/cua-driver manifests):
#
#   docker build -f libs/cua-spacesd/crates/cua-oidc/Dockerfile -t cua-oidc libs
#
# The image signs with AWS KMS (cargo feature `kms`); a `file:` key works
# too (self-hosted). TLS terminates in front of it (ingress); it serves
# plain HTTP on :8080. Base images and toolchain match cua-relay's
# Dockerfile: bump them together.
ARG RUST_IMAGE=rust:1.97.1-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97
ARG RUNTIME_IMAGE=debian:bookworm-slim@sha256:7ff6ba31527b9e336162fa061746c366c4b2032f4d2cff2078bea43f78efe3f8

FROM ${RUST_IMAGE} AS build
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
RUN cd cua-spacesd \
 && cargo build --release --locked -p cua-oidc --features kms \
 && install -m 0755 target/release/cua-oidc /usr/local/bin/cua-oidc

# CA bundle from the pinned Rust image (no apt at build time); the same
# eMudhra roots disabled as in cua-relay's image (CUA-1948).
FROM ${RUST_IMAGE} AS certs
RUN sed -i -E 's#^(mozilla/(emSign_ECC_Root_CA_-_G3|Comodo_AAA_Services_root)\.crt)$#!\1#' /etc/ca-certificates.conf \
 && update-ca-certificates --fresh

FROM ${RUNTIME_IMAGE}
COPY --from=certs /usr/share/ca-certificates /usr/share/ca-certificates
COPY --from=certs /etc/ca-certificates.conf /etc/ca-certificates.conf
COPY --from=certs /etc/ssl/certs /etc/ssl/certs
COPY --from=build /usr/local/bin/cua-oidc /usr/local/bin/cua-oidc
USER 65532:65532
EXPOSE 8080
ENV CUA_OIDC_LISTEN=0.0.0.0:8080
ENTRYPOINT ["/usr/local/bin/cua-oidc"]
