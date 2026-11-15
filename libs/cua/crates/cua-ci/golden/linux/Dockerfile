# The Linux golden image of `cua ci`: what a CI container or VM has
# installed before any job. One recipe for both architectures; build it
# for each (`cua build --platform linux/arm64` and `linux/amd64`, or the
# refresh job's two builds) and publish the two tags of one digest.
#
#   docker buildx build --platform linux/arm64,linux/amd64 \
#     --build-arg NODE_VERSION=22 --build-arg PNPM_VERSION=10.4.1 \
#     --build-arg RUST_VERSION=1.90.0 -t ghcr.io/trycua/ci-linux:<digest> .
#
# It needs the network (apt, the Node and Rust installers); it is built by
# the refresh job only, never in a job's path. The guest runs
# cua-spacesd like every Space image; the actions runner is unpacked by
# the controller from its tarball cache (so the image never carries a
# stale runner), unless a build bakes it in as RUNNER_VERSION.
FROM ubuntu:24.04

ARG TARGETARCH
ARG NODE_VERSION=22
ARG PNPM_VERSION=latest
ARG RUST_VERSION=stable
ARG DEBIAN_FRONTEND=noninteractive

# Tools a CI job expects from a hosted Ubuntu runner, and nothing else.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl git git-lfs gnupg jq unzip zip xz-utils \
      build-essential pkg-config cmake ninja-build libssl-dev libicu74 \
      python3 python3-pip python3-venv perl rsync openssh-client sudo tzdata \
 && rm -rf /var/lib/apt/lists/*

# CUA-1948: the compromised eMudhra intermediates. Ubuntu's ca-certificates >= 20260816ubuntu0.<release>.1 drops the
# roots they chain to; disable both roots explicitly until the base image ships it (a no-op once they are gone).
RUN sed -i -E 's#^(mozilla/(emSign_ECC_Root_CA_-_G3|Comodo_AAA_Services_root)\.crt)$#!\1#' /etc/ca-certificates.conf \
 && update-ca-certificates

# A job user with passwordless sudo, as the hosted runners have.
RUN useradd -m -s /bin/bash runner && echo 'runner ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/runner
USER runner
ENV HOME=/home/runner
WORKDIR /home/runner

# Node and pnpm (corepack ships with Node).
USER root
RUN curl -fsSL "https://deb.nodesource.com/setup_${NODE_VERSION}.x" | bash - \
 && apt-get install -y --no-install-recommends nodejs \
 && rm -rf /var/lib/apt/lists/* \
 && corepack enable && corepack prepare "pnpm@${PNPM_VERSION}" --activate
USER runner

# Rust and sccache.
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal \
      --default-toolchain "${RUST_VERSION}" -c clippy -c rustfmt
ENV PATH="/home/runner/.cargo/bin:${PATH}"
RUN case "${TARGETARCH}" in arm64) a=aarch64;; amd64) a=x86_64;; *) exit 1;; esac \
 && v=0.10.0 \
 && curl -fsSL "https://github.com/mozilla/sccache/releases/download/v${v}/sccache-v${v}-${a}-unknown-linux-musl.tar.gz" \
    | tar -xz --strip-components=1 -C /tmp "sccache-v${v}-${a}-unknown-linux-musl/sccache" \
 && sudo install -m 0755 /tmp/sccache /usr/local/bin/sccache && rm /tmp/sccache

# The restore targets of the read-only caches (see perf::cachevol).
RUN mkdir -p .cache/cua-ci/pnpm-store .cache/cua-ci/sccache .cargo/registry .cargo/git

# Record what is in the image (the refresh job reads it back).
RUN { echo "node=$(node --version | tr -d v)"; echo "pnpm=$(pnpm --version)"; \
      echo "rustc=$(rustc --version | cut -d' ' -f2)"; echo "git=$(git --version | cut -d' ' -f3)"; } \
      > /home/runner/.cua-ci-versions
