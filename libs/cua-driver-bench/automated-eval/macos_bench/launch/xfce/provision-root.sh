#!/bin/bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# Provision a Linux XFCE desktop for the Bench v2 Linux rows (run as root, once). Ubuntu 24.04 with XFCE 4.18
# (xfwm4) on Xvfb at 1920x1080, e.g. Cua's `ghcr.io/trycua/linux:24.04` image, or plain Ubuntu with
# `apt-get install xfce4 xvfb dbus-x11 at-spi2-core` and a session that starts them.
#
# Installs the X11 tools the launcher uses, the apps of the multi-app tasks (LibreOffice Calc 26.2, GnuCash 5.17,
# Google Chrome), and the environment shims below. Usage: sudo ./provision-root.sh
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

apt-get update -qq
apt-get install -y -qq xdotool wmctrl psmisc x11-utils flatpak curl ca-certificates

# LibreOffice 26.2 from The Document Foundation's debs (the distro's version differs).
LO=26.2.7
if [ ! -x /opt/libreoffice26.2/program/soffice ]; then
  tmp=$(mktemp -d)
  curl -fsSL -o "$tmp/lo.tgz" "https://download.documentfoundation.org/libreoffice/stable/$LO/deb/x86_64/LibreOffice_${LO}_Linux_x86-64_deb.tar.gz"
  tar -C "$tmp" -xzf "$tmp/lo.tgz"
  apt-get purge -y -qq 'libreoffice*' >/dev/null 2>&1 || true
  apt-get install -y -qq "$tmp"/LibreOffice_${LO}*_deb/DEBS/*.deb
  rm -rf "$tmp"
fi
ln -sf /opt/libreoffice26.2/program/soffice /usr/local/bin/libreoffice

# Google Chrome (stable).
tmp=$(mktemp -d)
curl -fsSL -o "$tmp/chrome.deb" https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb
apt-get install -y -qq "$tmp/chrome.deb"
rm -rf "$tmp"

# GnuCash 5.17: the Flathub build (Ubuntu 24.04 ships an older GnuCash).
flatpak remote-add --if-not-exists --system flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install -y --system --noninteractive flathub org.gnucash.GnuCash
"$(dirname "$0")/shim-gnucash.sh"

# Chrome and Electron cannot use their sandbox inside a container (no user namespaces): run them with --no-sandbox
# through PATH shims. Chrome then shows its unsupported-flag infobar. Not needed on a full VM; skip it there.
if [ "${BENCH_NO_SANDBOX_SHIMS:-1}" = 1 ]; then
  printf '#!/bin/sh\nexec /usr/bin/google-chrome --no-sandbox "$@"\n' > /usr/local/bin/google-chrome
  chmod 755 /usr/local/bin/google-chrome
fi
echo PROVISION_ROOT_OK
