# Build/test image for the cua-spacesd server core on Linux. The repo is
# mounted at /repo; nothing is copied in.
FROM rust:1-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config perl make cmake clang libclang-dev protobuf-compiler libprotobuf-dev \
    libx11-dev libxcb1-dev libx11-xcb-dev libxkbcommon-dev libxkbcommon-x11-dev \
    libwayland-dev libdbus-1-dev libxrandr-dev libxi-dev libxtst-dev libxfixes-dev \
    libxdamage-dev libudev-dev libssl-dev python3 curl procps \
 && rm -rf /var/lib/apt/lists/*
RUN rustup component add rustfmt clippy
# cargo-nextest: the CI profile (.github/flaky/nextest.toml) retries a failed test once.
# Pinned so the image does not change under a new release. The tarball goes to a file
# because curl cannot retry a transfer it has already started writing to a pipe.
ARG NEXTEST_VERSION=0.9.151
RUN case "$(uname -m)" in aarch64) t=linux-arm ;; *) t=linux ;; esac \
 && curl -LsSf --retry 5 --retry-all-errors --retry-delay 5 --connect-timeout 20 --max-time 300 \
      -o /tmp/nextest.tar.gz "https://get.nexte.st/$NEXTEST_VERSION/$t" \
 && tar zxf /tmp/nextest.tar.gz -C /usr/local/cargo/bin \
 && rm /tmp/nextest.tar.gz \
 && cargo nextest --version
