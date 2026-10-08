# Headless runtime for the Linux smoke test (scripts/smoke/linux.sh): a stock
# Ubuntu 24.04 with Xvfb, a window manager, and the libraries Electron needs.
FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      xvfb xauth x11-utils xdotool openbox imagemagick dbus-x11 ca-certificates \
      libgtk-3-0t64 libnss3 libasound2t64 libgbm1 libxss1 libxtst6 libsecret-1-0 \
      libnotify4 libatspi2.0-0t64 libdrm2 libxkbfile1 libuuid1 libayatana-appindicator3-1 \
      file binutils desktop-file-utils xdg-utils \
    && rm -rf /var/lib/apt/lists/*
