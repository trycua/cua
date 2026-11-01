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

# CUA-1948: the compromised eMudhra intermediates. Ubuntu's ca-certificates >= 20260816ubuntu0.<release>.1 drops the
# roots they chain to; Debian has not shipped that yet, so disable both roots explicitly (a no-op once they are gone).
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && sed -i -E 's#^(mozilla/(emSign_ECC_Root_CA_-_G3|Comodo_AAA_Services_root)\.crt)$#!\1#' /etc/ca-certificates.conf \
 && update-ca-certificates \
 && rm -rf /var/lib/apt/lists/*
