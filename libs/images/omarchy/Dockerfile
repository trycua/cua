# syntax=docker/dockerfile:1.7
# omarchy: Omarchy (edge channel) on Arch Linux, with Omarchy's own
# cua-driver packages and cua-spacesd, for the Cua SDK, CLI and Spaces.
#
# What Omarchy edge ships for computer use (pkgs.omarchy.org/edge):
#   cua-driver-bin        /usr/bin/cua-driver (Cua Driver, repackaged by
#                         Omarchy; `update --apply` defers to pacman)
#   cua-hyprland-plugin   /usr/lib/cua/hyprland/cua-hyprland-plugin.so, the
#                         compositor plugin behind OS-level input: two agent
#                         seats (Cua-Agent, Cua-Agent-2) inside Hyprland,
#                         reached over $XDG_RUNTIME_DIR/hypr/<sig>/cua-input-v3*.sock
# Neither package enables itself. files/cua.lua turns them on the way
# omacom/omarchy#11342 ("Add Cua to Install > AI") does: the plugin is loaded
# at Hyprland start with plugin:cua:enabled, and CUA_DRIVER_RS_ENABLE_WAYLAND=1
# is in the session environment.
#
# This file builds the guest tree only. Hyprland needs a DRM/GBM device, which
# docker and gVisor do not provide, so there is no container variant: the tree
# is turned into a VM disk by omarchy/vm.Dockerfile + common/disk-builder
# (`cua image build libs/images/omarchy`), published as <repo>:<tag>-disk.
#
# Build context: libs/images. amd64 only (Omarchy and its packages are x86_64).
#   CUA_SPACESD_SOURCE local|release|none, as in libs/images/linux.
#   OMARCHY_PACKAGES   desktop (default: the Omarchy desktop without the
#                      heavyweight apps) | full (omarchy-base.packages).
ARG ARCH_BASE=archlinux:base-20260920.0.596911@sha256:f3691b4dde62ba4c4b6f0ae2c1fbf28e8c0c8c4b9a35c7e06dc1f70e21aa29f6
FROM ${ARCH_BASE} AS rootfs

ARG TARGETARCH
ARG CUA_SPACESD_SOURCE=local
ARG CUA_SPACESD_VERSION=0.1.0
ARG CUA_SPACESD_RELEASE_URL=https://github.com/trycua/cua/releases/download/cua-spacesd-v{version}/cua-spacesd-linux-{arch}.tar.gz
ARG OMARCHY_PACKAGES=desktop
ARG OMARCHY_KEYRING=omarchy-keyring-20251027-1-any.pkg.tar.zst
ARG OMARCHY_KEYRING_SHA256=9f45936fb1e0a13c6a7b5b15bcd27673465f8c51a8f68b8e1a6d46d586db588a
# ARCH_SNAPSHOT_DATE pins core, extra and multilib to an Arch Linux Archive
# day snapshot, https://archive.archlinux.org/repos/<date>/, for the build.
# cua-hyprland-plugin (Omarchy edge) pins the exact versions it was linked
# against (glibc, hyprland, aquamarine, libgcc, ...), and Arch moves past them:
# glibc went from 2.44+r24 to r50 on 2026-09-27, then the plugin was rebuilt
# against r50 on 2026-09-29. A fixed date broke both times. `auto` (the
# default) resolves the day from the plugin itself (files/arch-snapshot.sh):
# the newest archive day, up to two weeks before its build, that serves every
# version it pins. A YYYY/MM/DD date pins that day; empty builds from
# mirror.omarchy.org. The Omarchy edge repo has no snapshots and is not
# pinned. The finished guest keeps Omarchy's own edge mirrorlist either way.
ARG ARCH_SNAPSHOT_DATE=auto
ENV LANG=C.UTF-8 LC_ALL=C.UTF-8 TZ=Etc/UTC

RUN [ "$TARGETARCH" = amd64 ] || { echo "Omarchy is x86_64 only (got $TARGETARCH)" >&2; exit 1; }

# The Omarchy edge channel, exactly as `omarchy-channel-set edge` configures
# it (omarchy-settings: default/pacman/pacman-edge.conf + mirrorlist-edge),
# plus DisableSandbox/no CheckSpace for building under docker and qemu-user.
# The build-time mirrorlist is the ARCH_SNAPSHOT_DATE archive when set.
COPY omarchy/files/pacman-edge.conf /etc/pacman.conf
COPY omarchy/files/arch-snapshot.sh /tmp/arch-snapshot.sh
RUN echo 'Server = https://mirror.omarchy.org/$repo/os/$arch' >/etc/pacman.d/mirrorlist.omarchy \
 && if [ "$ARCH_SNAPSHOT_DATE" = auto ]; then \
      ARCH_SNAPSHOT_DATE=$(sh /tmp/arch-snapshot.sh) || exit 1; \
      echo "Arch archive day for cua-hyprland-plugin: $ARCH_SNAPSHOT_DATE"; fi \
 && rm -f /tmp/arch-snapshot.sh \
 && if [ -n "$ARCH_SNAPSHOT_DATE" ]; then \
      echo "Server = https://archive.archlinux.org/repos/$ARCH_SNAPSHOT_DATE/\$repo/os/\$arch" >/etc/pacman.d/mirrorlist; \
    else cp /etc/pacman.d/mirrorlist.omarchy /etc/pacman.d/mirrorlist; fi \
 && pacman-key --init \
 && pacman-key --populate archlinux \
 && pacman -Sy --noconfirm archlinux-keyring \
 && curl -fsSL --retry 5 -o "/tmp/$OMARCHY_KEYRING" "https://pkgs.omarchy.org/edge/x86_64/$OMARCHY_KEYRING" \
 && curl -fsSL --retry 5 -o "/tmp/$OMARCHY_KEYRING.sig" "https://pkgs.omarchy.org/edge/x86_64/$OMARCHY_KEYRING.sig" \
 && echo "$OMARCHY_KEYRING_SHA256  /tmp/$OMARCHY_KEYRING" | sha256sum -c - \
 && install -d /usr/share/pacman/keyrings \
 && for f in omarchy.gpg omarchy-trusted omarchy-revoked; do \
      bsdtar -xOf "/tmp/$OMARCHY_KEYRING" "usr/share/pacman/keyrings/$f" >"/usr/share/pacman/keyrings/$f"; done \
 && pacman-key --populate omarchy \
 && pacman-key --verify "/tmp/$OMARCHY_KEYRING.sig" "/tmp/$OMARCHY_KEYRING" \
 && rm /usr/share/pacman/keyrings/omarchy* \
 && pacman -U --noconfirm "/tmp/$OMARCHY_KEYRING" \
 && rm -f /tmp/omarchy-keyring*

# Omarchy edge (omarchy-dev + omarchy-settings-dev, what the edge channel
# installs) with its cua-driver packages, and the desktop set. qemu-user builds
# (amd64 on an arm64 host) occasionally fail a download or a hook; retry.
COPY omarchy/files/packages.desktop /tmp/packages.desktop
RUN set -eu; \
    pacman -Syu --noconfirm; \
    pkgs="omarchy-dev omarchy-settings-dev cua-driver-bin cua-hyprland-plugin $(grep -v '^#' /tmp/packages.desktop | xargs)"; \
    for attempt in 1 2 3; do pacman -S --noconfirm --needed $pkgs && break; \
      [ "$attempt" = 3 ] && exit 1; echo "pacman attempt $attempt failed; retrying" >&2; sleep 5; done; \
    if [ "$OMARCHY_PACKAGES" = full ]; then \
      pacman -S --noconfirm --needed $(sed 's/#.*//' /usr/share/omarchy/install/omarchy-base.packages | xargs); fi; \
    pacman -Q omarchy-dev omarchy-settings-dev cua-driver-bin cua-hyprland-plugin hyprland; \
    test -x /usr/bin/cua-driver; test -f /usr/lib/cua/hyprland/cua-hyprland-plugin.so; \
    yes | pacman -Scc >/dev/null; rm -f /tmp/packages.desktop; \
    mv /etc/pacman.d/mirrorlist.omarchy /etc/pacman.d/mirrorlist

# The desktop user (uid 1000, like every libs/images desktop), seeded from
# Omarchy's /etc/skel and its unattended setup: the default theme applied
# headless, first-run and finalize marked done (they are interactive), and the
# screen kept awake. files/cua.lua is included last from hyprland.lua.
COPY omarchy/files/cua.lua /usr/share/cua/omarchy/cua.lua
COPY omarchy/files/uwsm-env /tmp/uwsm-env
RUN set -eu; \
    useradd -m -u 1000 -s /bin/bash -G wheel,video,input,audio cua; \
    install -d -o cua -g cua -m 0755 /volume; \
    passwd -l cua >/dev/null; \
    echo 'cua ALL=(ALL) NOPASSWD:ALL' >/etc/sudoers.d/90-cua; chmod 0440 /etc/sudoers.d/90-cua; \
    cp -a /etc/skel/. /home/cua/; \
    H=/home/cua; \
    install -d "$H/.config/uwsm" "$H/.local/state/omarchy/done" "$H/.local/state/omarchy/indicators" "$H/.cache/cua-driver"; \
    install -m 0644 /tmp/uwsm-env "$H/.config/uwsm/env"; rm /tmp/uwsm-env; \
    touch "$H/.local/state/omarchy/done/first-run-user" "$H/.local/state/omarchy/done/finalize-user" \
          "$H/.local/state/omarchy/indicators/stay-awake"; \
    printf '\n-- Cua sandbox wiring (libs/images/omarchy).\ndofile("/usr/share/cua/omarchy/cua.lua")\n' >>"$H/.config/hypr/hyprland.lua"; \
    chown -R cua:cua "$H"; \
    su - cua -c 'OMARCHY_PATH=/usr/share/omarchy OMARCHY_SETUP_CONTEXT=iso-chroot OMARCHY_THEME_HEADLESS=1 omarchy-theme-set "Tokyo Night"' \
      || echo "theme-set failed; Omarchy falls back to its defaults" >&2; \
    install -d -m 0755 /etc/sddm.conf.d /var/lib/sddm; \
    printf '[Autologin]\nUser=cua\nSession=omarchy.desktop\nRelogin=true\n' >/etc/sddm.conf.d/50-cua-autologin.conf; \
    printf '[Last]\nSession=omarchy.desktop\nUser=cua\n' >/var/lib/sddm/state.conf; \
    chown -R sddm:sddm /var/lib/sddm 2>/dev/null || true

# cua-spacesd (the same binary libs/images/linux bakes).
RUN --mount=type=bind,source=linux/dist,target=/dist \
    set -eu; bin=/usr/local/bin/cua-spacesd; \
    case "$CUA_SPACESD_SOURCE" in \
      local) src="/dist/$TARGETARCH/cua-spacesd"; \
        [ -f "$src" ] || { echo "CUA_SPACESD_SOURCE=local but $src is missing; run linux/build-spacesd-linux.sh amd64" >&2; exit 1; }; \
        install -m 0755 "$src" "$bin" ;; \
      release) url="$(echo "$CUA_SPACESD_RELEASE_URL" | sed "s/{version}/$CUA_SPACESD_VERSION/g; s/{arch}/x86_64/g")"; \
        echo "fetching $url"; curl -fsSL "$url" | tar -xz -C /usr/local/bin cua-spacesd; chmod 0755 "$bin" ;; \
      none) echo "building without cua-spacesd" ;; \
      *) echo "unknown CUA_SPACESD_SOURCE=$CUA_SPACESD_SOURCE" >&2; exit 1 ;; \
    esac; \
    if [ -x "$bin" ]; then ldd "$bin" | grep 'not found' && exit 1 || true; fi; \
    mkdir -p /etc/cua-image; echo "$CUA_SPACESD_SOURCE" >/etc/cua-image/spacesd-source

# Token handling, Fleet claim token sync and the in-guest fixtures are shared
# with libs/images/linux; the session glue and units are Omarchy's own.
COPY linux/files/ensure-env-token.sh linux/files/env-token-mode.sh \
     linux/files/start-spacesd.sh linux/files/start-token-sync.sh \
     omarchy/files/bin/ /opt/cua/bin/
COPY linux/files/pipewire/ /etc/pipewire/pipewire.conf.d/
COPY linux/files/systemd/cua-env-token-sync.service linux/files/systemd/run-cua.mount \
     linux/files/systemd/cua-volume-helper.service \
     omarchy/files/systemd/ /usr/lib/systemd/system/
COPY omarchy/files/systemd-user/ /usr/lib/systemd/user/
COPY linux/files/udev/ /usr/lib/udev/rules.d/
COPY linux/fixtures/ /opt/cua/fixtures/
COPY omarchy/image.json /etc/cua-image/image.json
RUN set -eu; \
    ln -s /opt/cua/fixtures/bin/cua-fixtures /usr/local/bin/cua-fixtures; \
    printf 'd /run/cua-desktop 0755 cua cua -\n' >/usr/lib/tmpfiles.d/cua-omarchy.conf; \
    printf 'cua-env-token-sync.service\ncua-volume-helper.service\ncua-spacesd.service\ncua-desktop-env.path\nsddm.service\n' >/etc/cua-image/vm-units; \
    echo vm >/etc/cua-image/variant; \
    systemctl --global enable cua-driver.service; \
    python3 -m py_compile /opt/cua/fixtures/*.py; \
    pacman -Q | sort >/etc/cua-image/packages; \
    pacman -Q omarchy-dev cua-driver-bin cua-hyprland-plugin hyprland >/etc/cua-image/omarchy-versions

COPY common/tools/cua-image-manifest /opt/cua/bin/cua-image-manifest
ARG CUA_IMAGE_SOURCE_REVISION=
RUN /opt/cua/bin/cua-image-manifest generate --image-json /etc/cua-image/image.json \
      --variant rootfs --arch "$TARGETARCH" --spacesd-source "$(cat /etc/cua-image/spacesd-source)" \
      --source-revision "$CUA_IMAGE_SOURCE_REVISION" --out /etc/cua-image/manifest.json

ENV HOME=/root
EXPOSE 3211 3212/udp
