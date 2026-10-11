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
