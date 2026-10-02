# syntax=docker/dockerfile:1.7
# bench-web: one Linux image for the browser benchmarks (MiniWoB++,
# WebVoyager, Online-Mind2Web, WebGym), plus bench-ui so the Cua Bench
# registry's desktop tasksets (their pywebview apps) run on the same desktop.
#
# FROM the libs/images/linux rootfs (BASE_IMAGE): built from the same commit
# by default, or a published ghcr.io/trycua/linux@sha256:... pin
# (CUA_BENCH_BASE_IMAGE). XFCE on Xvfb :1, cua-spacesd on :3211 with the
# HTML5 viewer, plus:
#   - Chromium (open-source build, BSD-3-Clause, from the xtradeb/apps PPA,
#     pinned by version and signing key) with CDP on 127.0.0.1:9223,
#     published on 0.0.0.0:9222 by socat. Not Playwright's download: that is
#     Google-branded Chrome for Testing, which this public image must not ship;
#   - bench-web-ctl on :7000: a small HTTP API over CDP (health, reset,
#     navigate, eval) so adapters reach the browser the same way locally and
#     on Fleet (plain HTTP through the service gateway);
#   - MiniWoB++ (MIT) at /opt/miniwob, served on :7560;
#   - bench-ui (libs/python/bench-ui) and pywebview (GTK + WebKit2) in the
#     system Python, which cua-bench's remote session imports.
# The VM variant is derived from this image by libs/images/build.sh
# (common/vm enables the units in /etc/cua-image/vm-units).
#
# Build context: libs/images, plus the named context `benchui`
# (libs/python/bench-ui): scripts/bench-images/build-docker.sh passes both.
ARG BASE_IMAGE
FROM ${BASE_IMAGE}
ARG TARGETARCH
# The Chromium the base (libs/images/linux, whose Dockerfile pins it per
# arch) installed, so the two never drift apart (the PPA keeps only its
# newest build per arch). CHROMIUM_VERSION, when set, wins.
ARG CHROMIUM_VERSION=
ARG CHROMIUM_PPA_KEY=5301FA4FD93244FBC6F6149982BB6851C64F6880
ARG PYWEBVIEW_VERSION=6.2.1
ARG WEBSOCKET_CLIENT_VERSION=1.9.2
ARG MINIWOB_REPO=https://github.com/Farama-Foundation/miniwob-plusplus.git
ARG MINIWOB_COMMIT=a7658beb68e1d3572329e06734c5822bd284ddc8
ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
 && for attempt in 1 2 3; do apt-get install -y --no-install-recommends \
      python3-pip python3-venv socat git gir1.2-webkit2-4.1 python3-psutil python3-aiohttp \
      fonts-noto-core fonts-noto-cjk fonts-noto-color-emoji \
      && break; echo "apt attempt $attempt failed; reconfiguring" >&2; dpkg --configure -a || true; \
      [ "$attempt" = 3 ] && exit 1; done \
 && rm -rf /var/lib/apt/lists/*

# Chromium from the xtradeb/apps PPA: only the chromium packages may come from
# it (everything else stays Ubuntu's), at exactly CHROMIUM_VERSION, and the
# key must have the pinned fingerprint. The key is vendored
# (common/apt-keys/xtradeb-apps.asc, shared with libs/images/linux), not
# fetched from keyserver.ubuntu.com, so a keyserver outage cannot break the build.
RUN --mount=type=bind,source=common/apt-keys/xtradeb-apps.asc,target=/tmp/xtradeb-apps.asc \
    install -d -m 0755 /etc/apt/keyrings \
 && gpg --dearmor </tmp/xtradeb-apps.asc >/etc/apt/keyrings/xtradeb-apps.gpg \
 && gpg --show-keys --with-colons /etc/apt/keyrings/xtradeb-apps.gpg | grep -q "^fpr:::::::::${CHROMIUM_PPA_KEY}:" \
 && echo "deb [signed-by=/etc/apt/keyrings/xtradeb-apps.gpg] https://ppa.launchpadcontent.net/xtradeb/apps/ubuntu noble main" \
      >/etc/apt/sources.list.d/xtradeb-apps.list \
 && printf 'Package: *\nPin: origin ppa.launchpadcontent.net\nPin-Priority: -1\n\nPackage: chromium chromium-common chromium-sandbox\nPin: origin ppa.launchpadcontent.net\nPin-Priority: 1001\n' \
      >/etc/apt/preferences.d/xtradeb-chromium \
 && pin="${CHROMIUM_VERSION:-$(dpkg-query -W -f='${Version}' chromium 2>/dev/null || true)}" && test -n "$pin" \
 && apt-get update \
 && for attempt in 1 2 3; do apt-get install -y --no-install-recommends \
      "chromium=${pin}" "chromium-common=${pin}" && break; \
      echo "apt attempt $attempt failed; reconfiguring" >&2; dpkg --configure -a || true; \
      [ "$attempt" = 3 ] && exit 1; done \
 && rm -rf /var/lib/apt/lists/* \
 && chromium --version | tee /etc/cua-image/chromium-version

# CDP client for bench-web-ctl.
RUN python3 -m venv /opt/bench-web/venv \
 && /opt/bench-web/venv/bin/pip install --no-cache-dir "websocket-client==${WEBSOCKET_CLIENT_VERSION}"

# MiniWoB++ at a pinned commit (only the task pages and the license).
RUN git clone --filter=blob:none --no-checkout "$MINIWOB_REPO" /tmp/miniwob \
 && git -C /tmp/miniwob checkout -q "$MINIWOB_COMMIT" \
 && test "$(git -C /tmp/miniwob rev-parse HEAD)" = "$MINIWOB_COMMIT" \
 && mkdir -p /opt/miniwob \
 && cp -r /tmp/miniwob/miniwob/html /opt/miniwob/html \
 && cp /tmp/miniwob/LICENSE /opt/miniwob/LICENSE \
 && echo "$MINIWOB_COMMIT" >/opt/miniwob/COMMIT \
 && rm -rf /tmp/miniwob \
 && test -f /opt/miniwob/html/miniwob/click-button.html

# bench-ui + pywebview in the system Python (cua-bench runs `python3 -c`
# snippets that import bench_ui inside the guest).
COPY --from=benchui . /opt/bench-ui-src
RUN pip install --no-cache-dir --break-system-packages "pywebview==${PYWEBVIEW_VERSION}" /opt/bench-ui-src \
 && rm -rf /opt/bench-ui-src \
 && python3 -c "import bench_ui, webview, gi; print('bench_ui ok')" \
 && ls /usr/lib/*/girepository-1.0/WebKit2-4.1.typelib
# (WebKit itself is not imported here: its JIT segfaults under the qemu-user
# emulation foreign-arch builds use. The smoke opens a real bench-ui window.)

COPY bench/web/files/bench-web-ctl.py /opt/bench-web/bench-web-ctl.py
COPY bench/web/files/start-chromium.sh bench/web/files/start-cdp-proxy.sh /opt/bench-web/
COPY bench/web/files/supervisor/bench-web.conf /etc/supervisor/conf.d/bench-web.conf
COPY bench/web/files/systemd/ /usr/lib/systemd/system/
COPY bench/web/bench.json /etc/cua-image/bench.json
# What this image claims (bench-web's image.json replaces the base's), baked
# into /etc/cua-image/manifest.json for `cua-spacesd doctor`; the VM layer
# rewrites the variant fields.
COPY bench/web/image.json /etc/cua-image/image.json
ARG CUA_IMAGE_SOURCE_REVISION=
RUN chmod 0755 /opt/bench-web/*.sh /opt/bench-web/bench-web-ctl.py \
 && printf '\n[include]\nfiles = /etc/supervisor/conf.d/*.conf\n' >>/etc/supervisor/supervisord.conf \
 && printf 'bench-web-chromium.service\nbench-web-cdp.service\nbench-web-ctl.service\nbench-web-miniwob.service\n' >>/etc/cua-image/vm-units \
 && /opt/bench-web/venv/bin/python -m py_compile /opt/bench-web/bench-web-ctl.py \
 && /opt/cua/bin/cua-image-manifest generate --image-json /etc/cua-image/image.json \
      --variant rootfs --arch "$TARGETARCH" --spacesd-source "$(cat /etc/cua-image/spacesd-source)" \
      --source-revision "$CUA_IMAGE_SOURCE_REVISION" --out /etc/cua-image/manifest.json

# 7000 bench-web-ctl, 9222 Chrome CDP, 7560 MiniWoB++ (plus the base ports).
EXPOSE 7000 9222 7560
