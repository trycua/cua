# syntax=docker/dockerfile:1.7
# libs/images/linux (ghcr.io/trycua/linux): the default recommended "Spaces image",
# in two tiers
# (build one with `--target`; the default is the last stage, full):
#
#   slim   ghcr.io/trycua/linux:24.04-slim[-disk]  the minimum for
#          `cua-spacesd doctor --strict` with every feature: no Firefox
#   full   ghcr.io/trycua/linux:24.04[-disk]       FROM slim + dev tooling
#          (build-essential, git, gh, jq, ripgrep, Node LTS + pnpm, python3 +
#          uv, rustup stable, Go, the Docker CLI) + Firefox
#
# full is FROM slim, so the two share every slim layer in the registry.
#
# slim: Ubuntu 24.04 + XFCE on Xvfb (:1) + session D-Bus + AT-SPI + headless
# PipeWire (null sink cua_desktop, virtual source cua_mic)
# + Chromium (the browser cua-driver's browser tools drive over CDP)
# + cua-spacesd (3211: gRPC, media, and the HTML5 viewer at /viewer)
# + conformance fixtures under
# /opt/cua/fixtures. This file builds the container rootfs (docker / gVisor,
# supervisord as PID 1). The KubeVirt containerDisk is derived from THIS image
# by common/vm/Dockerfile + common/disk-builder (see ../build.sh), so both
# artifacts always carry the same guest.
#
# Build context: libs/images (so shared files under common/ are reachable).
#   docker buildx build -f linux/Dockerfile libs/images \
#     --target slim|full --build-arg CUA_SPACESD_SOURCE=local|release|none
#
# CUA_SPACESD_SOURCE
#   local    copy linux/dist/<TARGETARCH>/cua-spacesd from the
#            build context (produce it with build-spacesd-linux.sh). Missing
#            binary = build error.
#   release  download CUA_SPACESD_RELEASE_URL ({version} and {arch} =
#            x86_64|aarch64 expanded), a .tar.gz holding a top-level
#            `cua-spacesd` (.github/workflows/cd-cua-spacesd.yml).
#   none     no driver; the supervisor program idles. Used while the driver is
#            not buildable, and as a sanity check that nothing else depends on it.
# The older build args (CUA_GUESTD_* and CUA_ENV_DRIVER_* SOURCE/_VERSION/
# _RELEASE_URL) are still honoured when the CUA_SPACESD_* one is not given.
#
# Compat for one release, for both older names (cua-env-driver, cua-guestd):
# /usr/local/bin/<old> links to cua-spacesd, <old>.service is an alias of
# cua-spacesd.service, and /opt/cua/bin/start-<old>.sh links to
# start-spacesd.sh.
ARG UBUNTU_VERSION=24.04
FROM ubuntu:${UBUNTU_VERSION} AS slim

ARG TARGETARCH
# Empty = the older CUA_GUESTD_* / CUA_ENV_DRIVER_* arg, else the default in the RUN below
# (local; 0.1.0, keep in step with libs/cua-spacesd/VERSION; the GitHub
# release URL).
ARG CUA_SPACESD_SOURCE=
ARG CUA_SPACESD_VERSION=
ARG CUA_SPACESD_RELEASE_URL=
ARG CUA_GUESTD_SOURCE=
ARG CUA_GUESTD_VERSION=
ARG CUA_GUESTD_RELEASE_URL=
ARG CUA_ENV_DRIVER_SOURCE=
ARG CUA_ENV_DRIVER_VERSION=
ARG CUA_ENV_DRIVER_RELEASE_URL=
ENV DEBIAN_FRONTEND=noninteractive LANG=C.UTF-8 LC_ALL=C.UTF-8 TZ=Etc/UTC

# Desktop, display, a11y and test tooling. No snaps anywhere (the full tier's
# Firefox comes from Mozilla's APT repo, amd64 + arm64).
# `apt-get install || dpkg --configure -a` retries: foreign-arch builds under
# qemu-user (e.g. amd64 on an Apple Silicon Colima VM) intermittently segfault
# in python byte-compilation postinsts; re-running the configure step passes.
# Native builds never take the retry path.
# The last group is the runtime of Electron and GTK apps that teleport
# installs (VS Code, Slack, Discord, ...), so they start at once instead of
# resolving packages first. XFCE and Chromium already pull nearly all of it;
# listing it keeps it when they change, and the rest (libsecret, libxss,
# EGL) adds about 1.2 MB.
RUN apt-get update \
 && for attempt in 1 2 3; do apt-get install -y --no-install-recommends \
      ca-certificates curl gnupg sudo tzdata locales procps psmisc iproute2 fuse3 \
      netcat-openbsd less nano xz-utils \
      supervisor dbus dbus-x11 dbus-user-session \
      xfce4 xfce4-terminal xfwm4 xfdesktop4 xfce4-panel xfce4-settings thunar \
      greybird-gtk-theme adwaita-icon-theme-full fonts-dejavu-core fonts-liberation \
      xvfb \
      at-spi2-core libatk-adaptor libgail-common \
      python3 python3-gi python3-gi-cairo gir1.2-gtk-3.0 gir1.2-atspi-2.0 python3-pyatspi \
      x11-utils x11-xserver-utils x11-apps xauth xdotool wmctrl xclip imagemagick \
      libxtst6 libxi6 libxkbcommon0 libxkbcommon-x11-0 \
      pipewire pipewire-pulse pipewire-bin wireplumber pulseaudio-utils \
      libpulse0 libopus0 \
      libnss3 libnspr4 libsecret-1-0 libxkbfile1 libgbm1 libasound2t64 libatk-bridge2.0-0t64 \
      libgtk-3-0t64 libcups2t64 libxss1 libcurl3t64-gnutls libegl1 libgl1 libxrender1 xdg-utils \
      && break; echo "apt attempt $attempt failed; reconfiguring" >&2; dpkg --configure -a || true; \
      [ "$attempt" = 3 ] && exit 1; done \
 && rm -rf /var/lib/apt/lists/* \
 && mkdir -p /etc/systemd/user \
 && for u in pipewire.socket pipewire.service pipewire-pulse.socket pipewire-pulse.service \
      wireplumber.service filter-chain.service; do ln -sf /dev/null "/etc/systemd/user/$u"; done
# (The user-session PipeWire units are masked: cua-audio runs one headless
# instance for the desktop instead, see files/start-audio.sh.)

RUN install -d -m 0755 /etc/apt/keyrings

# Chromium from the xtradeb/apps PPA (open-source build, BSD-3-Clause; not
# Google-branded Chrome): only the chromium packages may come from it, at
# exactly CHROMIUM_VERSION, and the key must have the pinned fingerprint.
# The key is vendored (common/apt-keys/xtradeb-apps.asc), not fetched from
# keyserver.ubuntu.com, so a keyserver outage cannot break the build.
# cua-driver's browser_prepare launches /usr/lib/chromium/chromium (a
# root-owned package payload) with a driver-owned throwaway profile. The
# managed policy turns off first-run noise, sign-in and password saving.
# Per-arch pins: the PPA keeps only its newest build per arch, and the
# arches do not always move together. CHROMIUM_VERSION, when set, wins.
ARG CHROMIUM_VERSION=
ARG CHROMIUM_VERSION_AMD64=154.0.8037.92-1xtradeb1.2404.1
ARG CHROMIUM_VERSION_ARM64=154.0.8037.92-1xtradeb1.2404.1
ARG CHROMIUM_PPA_KEY=5301FA4FD93244FBC6F6149982BB6851C64F6880
RUN --mount=type=bind,source=common/apt-keys/xtradeb-apps.asc,target=/tmp/xtradeb-apps.asc \
    gpg --dearmor </tmp/xtradeb-apps.asc >/etc/apt/keyrings/xtradeb-apps.gpg \
 && gpg --show-keys --with-colons /etc/apt/keyrings/xtradeb-apps.gpg | grep -q "^fpr:::::::::${CHROMIUM_PPA_KEY}:" \
 && echo "deb [signed-by=/etc/apt/keyrings/xtradeb-apps.gpg] https://ppa.launchpadcontent.net/xtradeb/apps/ubuntu noble main" \
      >/etc/apt/sources.list.d/xtradeb-apps.list \
 && printf 'Package: *\nPin: origin ppa.launchpadcontent.net\nPin-Priority: -1\n\nPackage: chromium chromium-common chromium-sandbox\nPin: origin ppa.launchpadcontent.net\nPin-Priority: 1001\n' \
      >/etc/apt/preferences.d/xtradeb-chromium \
 && case "$TARGETARCH" in amd64) pin="$CHROMIUM_VERSION_AMD64" ;; arm64) pin="$CHROMIUM_VERSION_ARM64" ;; *) pin="" ;; esac \
 && pin="${CHROMIUM_VERSION:-$pin}" && test -n "$pin" \
 && apt-get update \
 && for attempt in 1 2 3; do apt-get install -y --no-install-recommends \
      "chromium=${pin}" "chromium-common=${pin}" && break; \
      echo "apt attempt $attempt failed; reconfiguring" >&2; dpkg --configure -a || true; \
      [ "$attempt" = 3 ] && exit 1; done \
 && rm -rf /var/lib/apt/lists/* \
 && mkdir -p /etc/chromium/policies/managed /etc/cua-image \
 && printf '%s\n' '{"BrowserSignin":0,"SyncDisabled":true,"PasswordManagerEnabled":false,' \
      '"PromotionsEnabled":false,"DefaultBrowserSettingEnabled":false,"MetricsReportingEnabled":false,' \
      '"PrivacySandboxPromptEnabled":false,"TranslateEnabled":false}' \
      >/etc/chromium/policies/managed/cua.json \
 && python3 -m json.tool /etc/chromium/policies/managed/cua.json >/dev/null \
 && test -x /usr/lib/chromium/chromium \
 && chromium --version | tee /etc/cua-image/chromium-version

# Shared desktop plumbing (same files as plain/ubuntu-xfce-vnc).
COPY common/desktop/ /opt/cua/desktop/
# /volume: where cua-spacesd mounts Cua Volume (FUSE, served by the client
# through /volume; see VolumeService). Owned by the desktop user.
RUN /opt/cua/desktop/install-desktop-user.sh cua 1000 \
 && install -d -o cua -g cua -m 0755 /volume \
 && sed -i 's/^WebBrowser=.*/WebBrowser=chromium/' /home/cua/.config/xfce4/helpers.rc \
 && ln -s /opt/cua/desktop/desktop-env.sh /usr/local/bin/desktop-env \
 && install -d /etc/systemd/system/cua-desktop.service.d \
 && printf '[Service]\nEnvironment=CUA_X_SERVER=xvfb\n' >/etc/systemd/system/cua-desktop.service.d/x-server.conf

# cua-spacesd.
RUN --mount=type=bind,source=linux/dist,target=/dist \
    set -eu; \
    bin=/usr/local/bin/cua-spacesd; \
    CUA_SPACESD_SOURCE="${CUA_SPACESD_SOURCE:-${CUA_GUESTD_SOURCE:-${CUA_ENV_DRIVER_SOURCE:-local}}}"; \
    CUA_SPACESD_VERSION="${CUA_SPACESD_VERSION:-${CUA_GUESTD_VERSION:-${CUA_ENV_DRIVER_VERSION:-0.1.0}}}"; \
    default_url='https://github.com/trycua/cua/releases/download/cua-spacesd-v{version}/cua-spacesd-linux-{arch}.tar.gz'; \
    CUA_SPACESD_RELEASE_URL="${CUA_SPACESD_RELEASE_URL:-${CUA_GUESTD_RELEASE_URL:-${CUA_ENV_DRIVER_RELEASE_URL:-$default_url}}}"; \
    case "$CUA_SPACESD_SOURCE" in \
      local) \
        src="/dist/$TARGETARCH/cua-spacesd"; \
        if [ ! -f "$src" ]; then \
          echo "CUA_SPACESD_SOURCE=local but $src is missing; run linux/build-spacesd-linux.sh or pass CUA_SPACESD_SOURCE=none" >&2; exit 1; \
        fi; \
        install -m 0755 "$src" "$bin" ;; \
      release) \
        case "$TARGETARCH" in amd64) arch=x86_64 ;; arm64) arch=aarch64 ;; *) arch="$TARGETARCH" ;; esac; \
        CUA_SPACESD_VERSION="${CUA_SPACESD_VERSION#cua-spacesd-v}"; CUA_SPACESD_VERSION="${CUA_SPACESD_VERSION#v}"; \
        if [ "$CUA_SPACESD_VERSION" = latest ]; then \
          CUA_SPACESD_VERSION="$(curl -fsSL 'https://api.github.com/repos/trycua/cua/releases?per_page=100' \
            | python3 -c 'import json,sys; print(next(r["tag_name"][len("cua-spacesd-v"):] for r in json.load(sys.stdin) if r["tag_name"].startswith("cua-spacesd-v") and not r["draft"] and not r["prerelease"]))')"; \
          echo "latest cua-spacesd release: $CUA_SPACESD_VERSION"; \
        fi; \
        url="$(echo "$CUA_SPACESD_RELEASE_URL" | sed "s/{version}/$CUA_SPACESD_VERSION/g; s/{arch}/$arch/g")"; \
        echo "fetching $url"; \
        curl -fsSL -o /tmp/cua-spacesd.tar.gz "$url"; \
        sum="$(curl -fsSL "$url.sha256" | cut -d' ' -f1)"; \
        echo "$sum  /tmp/cua-spacesd.tar.gz" | sha256sum -c -; \
        tar -xzf /tmp/cua-spacesd.tar.gz -C /usr/local/bin cua-spacesd; \
        rm /tmp/cua-spacesd.tar.gz; \
        chmod 0755 "$bin"; \
        echo "$CUA_SPACESD_VERSION" >/tmp/cua-spacesd-version ;; \
      none) echo "building without cua-spacesd" ;; \
      *) echo "unknown CUA_SPACESD_SOURCE=$CUA_SPACESD_SOURCE" >&2; exit 1 ;; \
    esac; \
    if [ -x "$bin" ]; then \
      ldd "$bin" | grep 'not found' && exit 1 || true; \
      ln -s cua-spacesd /usr/local/bin/cua-env-driver; \
      ln -s cua-spacesd /usr/local/bin/cua-guestd; \
    fi; \
    mkdir -p /etc/cua-image; \
    echo "$CUA_SPACESD_SOURCE" >/etc/cua-image/spacesd-source; \
    if [ -f /tmp/cua-spacesd-version ]; then mv /tmp/cua-spacesd-version /etc/cua-image/spacesd-release; fi; \
    ln -s spacesd-source /etc/cua-image/env-driver-source; \
    ln -s spacesd-source /etc/cua-image/guestd-source

COPY linux/files/supervisor/supervisord.conf /etc/supervisor/supervisord.conf
COPY linux/files/start-spacesd.sh linux/files/ensure-env-token.sh \
     linux/files/env-token-mode.sh linux/files/start-token-sync.sh \
     linux/files/start-audio.sh linux/files/cua-chromium /opt/cua/bin/
COPY linux/files/pipewire/ /etc/pipewire/pipewire.conf.d/
COPY linux/files/spacesd.healthcheck.sh /opt/cua/healthcheck.d/10-spacesd.sh
COPY common/desktop/systemd/ linux/files/systemd/ /usr/lib/systemd/system/
COPY linux/files/udev/ /usr/lib/udev/rules.d/
COPY linux/fixtures/ /opt/cua/fixtures/
COPY linux/image.json /etc/cua-image/image.json
RUN install -d /opt/cua/entrypoint.d \
 && printf '/opt/cua/bin/ensure-env-token.sh\n' >/opt/cua/entrypoint.d/10-env-token.sh \
 && printf '# The token now lives in /run/cua/env-token; keep it out of the environment\n# every supervisord program (desktop apps included) would inherit.\nunset CUA_ENV_TOKEN CUA_SPACESD_TOKEN CUA_GUESTD_TOKEN\n' \
      >/opt/cua/entrypoint.d/90-scrub-env-token.sh \
 && ln -s /opt/cua/fixtures/bin/cua-fixtures /usr/local/bin/cua-fixtures \
 && ln -s start-spacesd.sh /opt/cua/bin/start-env-driver.sh \
 && ln -s start-spacesd.sh /opt/cua/bin/start-guestd.sh \
 && ln -s cua-spacesd.service /usr/lib/systemd/system/cua-env-driver.service \
 && ln -s cua-spacesd.service /usr/lib/systemd/system/cua-guestd.service \
 && printf 'cua-desktop.service\ncua-audio.service\ncua-env-token-sync.service\ncua-volume-helper.service\ncua-spacesd.service\n' >/etc/cua-image/vm-units \
 && echo container >/etc/cua-image/variant \
 && python3 -m py_compile /opt/cua/fixtures/*.py

# /etc/cua-image/manifest.json: what this image claims (image.json `claims`
# plus the baked cua-spacesd's build-info). `cua-spacesd doctor` checks the
# running guest against it; the VM layer rewrites the variant fields.
COPY common/tools/cua-image-manifest /opt/cua/bin/cua-image-manifest
ARG CUA_IMAGE_SOURCE_REVISION=
RUN /opt/cua/bin/cua-image-manifest generate --image-json /etc/cua-image/image.json --tier slim \
      --variant rootfs --arch "$TARGETARCH" --spacesd-source "$(cat /etc/cua-image/spacesd-source)" \
      --source-revision "$CUA_IMAGE_SOURCE_REVISION" --out /etc/cua-image/manifest.json

ENV HOME=/root DISPLAY=:1 CUA_X_SERVER=xvfb CUA_AUDIO=true
# 3211 spacesd (gRPC + gRPC-Web, /media, /viewer), 3212/udp spacesd QUIC
# media, 18080 HTTP fixture (only while started).
EXPOSE 3211 3212/udp 18080
HEALTHCHECK --interval=10s --timeout=5s --start-period=30s --retries=12 \
  CMD ["/opt/cua/desktop/healthcheck.sh"]
STOPSIGNAL SIGTERM
CMD ["/opt/cua/desktop/entrypoint.sh"]

# ---------------------------------------------------------------------------
# full (the default tier): slim + developer tooling + Firefox.
# Every download is pinned by version and sha256; apt packages come from the
# Ubuntu 24.04 archive (and Docker's signed repo, key fingerprint checked,
# docker-ce-cli only, at exactly DOCKER_CLI_VERSION). Tools land in
# /usr/local/bin (thin wrappers where a tool needs its home), so they work in
# every session: supervisord programs, ssh, and the VM's systemd units.
# ---------------------------------------------------------------------------
FROM slim AS full
ARG TARGETARCH
ARG CUA_IMAGE_SOURCE_REVISION=
ARG NODE_VERSION=24.21.0
ARG NODE_SHA256_AMD64=fd8e59d5a511510f6a298afb548f18c7d2b1be404d8b4a27d94fbe49f56cb2d6
ARG NODE_SHA256_ARM64=6ad1325edbdb5649c379b75a237147a666c95d4f9ae8d340fef2d1575d289ad2
ARG PNPM_VERSION=12.6.0
ARG GO_VERSION=1.27.1
ARG GO_SHA256_AMD64=63d339f0da5ab53635a56f2490a7984dfe12dfcff22ad749f63edaf590168445
ARG GO_SHA256_ARM64=3450b45a3f9ee8568792736a5c5e70a1f2e9b36c35a8f74958c03e51d7d92bec
ARG UV_VERSION=0.12.19
ARG UV_SHA256_AMD64=23bf5552d220e0842b65c862097b2ebaeba0064b74eda5e565e77fd25969d8c8
ARG UV_SHA256_ARM64=0804e9b164c64b6914182d5920c08551958a095986f10a3731056df701126436
ARG GH_VERSION=2.101.0
ARG GH_SHA256_AMD64=9bca2d1c16825f109907a23307628a2f0698fbf99662b73a5cf0b020293072b8
ARG GH_SHA256_ARM64=b57e8063f18862647c9d22727c32e9da1b963f8bf9db648fe123a6975695640f
ARG RUSTUP_VERSION=1.29.1
ARG RUSTUP_SHA256_AMD64=dda7234360b7f578ca8b0ddcb80145646fa61a67c1720a5abc7051b35c9fcb71
ARG RUSTUP_SHA256_ARM64=15f6e4ce9f583b929c996c91562bad6d4454f3281de858b02cdfdef615fac433
ARG RUST_TOOLCHAIN=1.98.1
ARG DOCKER_CLI_VERSION=5:29.8.1-1~ubuntu.24.04~noble
ARG DOCKER_APT_KEY=9DC858229FC7DD38854AE2D88D81803C0EBFCD88

# Build tools and the everyday CLI set, from the Ubuntu archive.
RUN apt-get update \
 && for attempt in 1 2 3; do apt-get install -y --no-install-recommends \
      build-essential pkg-config cmake ninja-build \
      git git-lfs jq ripgrep fd-find unzip zip python3-venv python3-pip \
      && break; echo "apt attempt $attempt failed; reconfiguring" >&2; dpkg --configure -a || true; \
      [ "$attempt" = 3 ] && exit 1; done \
 && rm -rf /var/lib/apt/lists/* \
 && ln -s /usr/bin/fdfind /usr/local/bin/fd

# Docker CLI only (no daemon): docker-ce-cli from Docker's signed repo.
RUN curl -fsSL https://download.docker.com/linux/ubuntu/gpg | gpg --dearmor >/etc/apt/keyrings/docker.gpg \
 && gpg --show-keys --with-colons /etc/apt/keyrings/docker.gpg | grep -q "^fpr:::::::::${DOCKER_APT_KEY}:" \
 && echo "deb [signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/ubuntu noble stable" \
      >/etc/apt/sources.list.d/docker.list \
 && printf 'Package: *\nPin: origin download.docker.com\nPin-Priority: -1\n\nPackage: docker-ce-cli\nPin: origin download.docker.com\nPin-Priority: 1001\n' \
      >/etc/apt/preferences.d/docker-cli \
 && apt-get update \
 && apt-get install -y --no-install-recommends "docker-ce-cli=${DOCKER_CLI_VERSION}" \
 && rm -rf /var/lib/apt/lists/* \
 && docker --version

# Pinned tarballs, each checked against its sha256 before it is unpacked.
RUN set -eu; \
    case "$TARGETARCH" in \
      amd64) node_arch=x64; go_sha="$GO_SHA256_AMD64"; node_sha="$NODE_SHA256_AMD64"; uv_t=x86_64; uv_sha="$UV_SHA256_AMD64"; gh_sha="$GH_SHA256_AMD64"; ru_t=x86_64; ru_sha="$RUSTUP_SHA256_AMD64" ;; \
      arm64) node_arch=arm64; go_sha="$GO_SHA256_ARM64"; node_sha="$NODE_SHA256_ARM64"; uv_t=aarch64; uv_sha="$UV_SHA256_ARM64"; gh_sha="$GH_SHA256_ARM64"; ru_t=aarch64; ru_sha="$RUSTUP_SHA256_ARM64" ;; \
      *) echo "unsupported arch $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    fetch() { curl -fsSL --retry 5 -o "$1" "$2" && echo "$3  $1" | sha256sum -c -; }; \
    cd /tmp; \
    fetch node.tar.xz "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-${node_arch}.tar.xz" "$node_sha"; \
    mkdir -p /usr/local/lib/nodejs && tar -xJf node.tar.xz -C /usr/local/lib/nodejs --strip-components=1; \
    for b in node npm npx corepack; do ln -sf "/usr/local/lib/nodejs/bin/$b" "/usr/local/bin/$b"; done; \
    fetch go.tar.gz "https://go.dev/dl/go${GO_VERSION}.linux-${TARGETARCH}.tar.gz" "$go_sha"; \
    tar -xzf go.tar.gz -C /usr/local; ln -sf /usr/local/go/bin/go /usr/local/bin/go; ln -sf /usr/local/go/bin/gofmt /usr/local/bin/gofmt; \
    fetch uv.tar.gz "https://github.com/astral-sh/uv/releases/download/${UV_VERSION}/uv-${uv_t}-unknown-linux-gnu.tar.gz" "$uv_sha"; \
    tar -xzf uv.tar.gz --strip-components=1 -C /usr/local/bin "uv-${uv_t}-unknown-linux-gnu/uv" "uv-${uv_t}-unknown-linux-gnu/uvx"; \
    fetch gh.tar.gz "https://github.com/cli/cli/releases/download/v${GH_VERSION}/gh_${GH_VERSION}_linux_${TARGETARCH}.tar.gz" "$gh_sha"; \
    tar -xzf gh.tar.gz --strip-components=2 -C /usr/local/bin "gh_${GH_VERSION}_linux_${TARGETARCH}/bin/gh"; \
    fetch rustup-init "https://static.rust-lang.org/rustup/archive/${RUSTUP_VERSION}/${ru_t}-unknown-linux-gnu/rustup-init" "$ru_sha"; \
    chmod +x rustup-init; \
    RUSTUP_HOME=/opt/rust/rustup CARGO_HOME=/opt/rust/cargo ./rustup-init -y --no-modify-path \
      --profile minimal --default-toolchain "$RUST_TOOLCHAIN" -c rustfmt -c clippy; \
    rm -f node.tar.xz go.tar.gz uv.tar.gz gh.tar.gz rustup-init; \
    chmod -R a+rX /opt/rust; \
    for b in rustup rustc cargo rustfmt cargo-fmt cargo-clippy clippy-driver rustdoc; do \
      printf '#!/bin/sh\n: "${RUSTUP_HOME:=/opt/rust/rustup}"\nexport RUSTUP_HOME\nexec /opt/rust/cargo/bin/%s "$@"\n' "$b" >"/usr/local/bin/$b"; \
      chmod 0755 "/usr/local/bin/$b"; \
    done; \
    COREPACK_HOME=/opt/corepack COREPACK_ENABLE_DOWNLOAD_PROMPT=0 corepack install -g "pnpm@${PNPM_VERSION}"; \
    chmod -R a+rX /opt/corepack; \
    printf '#!/bin/sh\n: "${COREPACK_HOME:=/opt/corepack}"\nexport COREPACK_HOME COREPACK_ENABLE_DOWNLOAD_PROMPT=0\nexec /usr/local/lib/nodejs/bin/corepack pnpm "$@"\n' >/usr/local/bin/pnpm; \
    chmod 0755 /usr/local/bin/pnpm; \
    node --version; pnpm --version; go version; uv --version; gh --version | head -1; rustc --version; cargo --version

# Firefox from Mozilla's APT repo (full tier only), with a policy that turns
# off updates, telemetry and first-run pages.
RUN curl -fsSL https://packages.mozilla.org/apt/repo-signing-key.gpg -o /etc/apt/keyrings/packages.mozilla.org.asc \
 && echo "deb [signed-by=/etc/apt/keyrings/packages.mozilla.org.asc] https://packages.mozilla.org/apt mozilla main" \
      >/etc/apt/sources.list.d/mozilla.list \
 && printf 'Package: *\nPin: origin packages.mozilla.org\nPin-Priority: 1000\n' >/etc/apt/preferences.d/mozilla \
 && apt-get update \
 && apt-get install -y --no-install-recommends firefox \
 && rm -rf /var/lib/apt/lists/* \
 && mkdir -p /usr/lib/firefox/distribution \
 && printf '%s\n' '{"policies":{"DisableAppUpdate":true,"DisableTelemetry":true,"DontCheckDefaultBrowser":true,' \
      '"OverrideFirstRunPage":"","OverridePostUpdatePage":"","NoDefaultBookmarks":true,"SkipTermsOfUse":true,' \
      '"UserMessaging":{"SkipOnboarding":true,"ExtensionRecommendations":false,"FeatureRecommendations":false,"MoreFromMozilla":false,"Locked":true},' \
      '"Preferences":{"browser.aboutwelcome.enabled":{"Value":false,"Status":"locked"},' \
      '"datareporting.policy.dataSubmissionPolicyBypassNotification":{"Value":true,"Status":"locked"},' \
      '"browser.shell.checkDefaultBrowser":{"Value":false,"Status":"locked"}}}}' \
      >/usr/lib/firefox/distribution/policies.json \
 && python3 -m json.tool /usr/lib/firefox/distribution/policies.json >/dev/null \
 && firefox --version

# Firefox is the full tier's default web browser, as before the tiers.
RUN sed -i 's/^WebBrowser=.*/WebBrowser=firefox/' /home/cua/.config/xfce4/helpers.rc

# The full tier's manifest: the same image.json, tier full (claims.tools).
RUN /opt/cua/bin/cua-image-manifest generate --image-json /etc/cua-image/image.json --tier full \
      --variant rootfs --arch "$TARGETARCH" --spacesd-source "$(cat /etc/cua-image/spacesd-source)" \
      --source-revision "$CUA_IMAGE_SOURCE_REVISION" --out /etc/cua-image/manifest.json
