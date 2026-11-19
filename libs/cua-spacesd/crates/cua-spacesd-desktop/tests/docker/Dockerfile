# Linux desktop test image for cua-spacesd-desktop conformance tests.
#
# Everything that drives a desktop (capture, XTest/XSendEvent input, AT-SPI,
# clipboard, audio) runs ONLY inside this container, never on the developer's
# host. The image bundles the Rust build toolchain plus a throwaway X11 desktop:
# Xvfb + openbox, the AT-SPI bus, PulseAudio (null sink), and the Tk/GTK
# fixture runtimes used by `tests/linux_desktop.rs`.
#
#   docker build -t cua-envdesktop-test libs/cua-spacesd/crates/cua-spacesd-desktop/tests/docker
#   libs/cua-spacesd/crates/cua-spacesd-desktop/tests/docker/run-tests.sh
#
# The first RUN matches the shared linuxdev build image layer so the Docker
# cache is reused.
FROM rust:1-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config perl make cmake clang libclang-dev \
    libx11-dev libxcb1-dev libx11-xcb-dev libxkbcommon-dev libxkbcommon-x11-dev \
    libwayland-dev libdbus-1-dev libfontconfig1-dev libfreetype6-dev \
    libxrandr-dev libxi-dev libxcursor-dev libxtst-dev libxfixes-dev libxdamage-dev \
    libgl1-mesa-dev libegl1-mesa-dev libasound2-dev libudev-dev libssl-dev \
    libgtk-3-dev libpipewire-0.3-dev libspa-0.2-dev \
 && rm -rf /var/lib/apt/lists/*
RUN rustup component add rustfmt clippy
RUN apt-get update && apt-get install -y --no-install-recommends \
    protobuf-compiler libprotobuf-dev nasm \
    xvfb xauth openbox x11-utils x11-xserver-utils xdotool \
    dbus dbus-x11 at-spi2-core libatk-adaptor \
    python3 python3-tk python3-gi gir1.2-gtk-3.0 \
    pulseaudio pulseaudio-utils \
    fonts-dejavu-core \
 && rm -rf /var/lib/apt/lists/*
ENV CARGO_BUILD_JOBS=6 \
    NO_AT_BRIDGE=0 \
    GTK_MODULES=gail:atk-bridge
