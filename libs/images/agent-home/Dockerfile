# syntax=docker/dockerfile:1.7
# agent-home: the lightest image a coding agent can live in and make child Spaces from.
#
# It is the slim Linux image (ghcr.io/trycua/linux:24.04-slim: cua-spacesd, the desktop and
# Chromium) plus the `cua` CLI. Inside a Space with child Spaces on, `cua spaces create`, `ls`
# and `delete` reach the Space's spawn channel on their own (nothing to configure, no token in
# the image), so an agent here can create, use and delete child Spaces within its tree's budget.
# The network conformance suite (libs/cua/scripts/conformance) makes its children case run
# in it. One definition, two outputs like the linux image: a container rootfs (docker, gVisor,
# microVMs) and, from the same rootfs, a VM disk (QEMU, KubeVirt).
#
# Where `cua` comes from (CUA_CLI_SOURCE), the way libs/images/linux gets cua-spacesd:
#   local    copy agent-home/dist/<TARGETARCH>/cua from the build context (produce it with
#            agent-home/build-cua-linux.sh). The default.
#   release  download cua-cli-<version>-linux-<x64|arm64>.tar.gz from the cua-sdk-v<version>
#            GitHub release (CUA_CLI_VERSION, or `latest`; CUA_CLI_RELEASE_URL has {version}
#            and {arch} placeholders). Pass CUA_CLI_SHA256 to check the archive.
#   none     no CLI (the image is then the slim image).
# The bind mount needs agent-home/dist to exist, also for `release` (`mkdir -p agent-home/dist`).
# The spawn channel is served by the guest's cua-spacesd: the base image's must be one that has it
# (cua-spacesd 0.10.1 or later). CUA_SPACESD_SOURCE replaces it: `base` keeps the base image's,
# `local` copies agent-home/dist/<arch>/cua-spacesd (linux/build-spacesd-linux.sh builds it into
# linux/dist; copy it over), `release` downloads CUA_SPACESD_VERSION from the cua-spacesd release
# like libs/images/linux does.
# The version must be one with child Spaces (cua-sdk 0.10.1 or later): the CLI of an older
# release does not know the spawn channel.
ARG BASE=ghcr.io/trycua/linux:24.04-slim
FROM ${BASE}
ARG TARGETARCH
ARG CUA_CLI_SOURCE=local
ARG CUA_CLI_VERSION=
ARG CUA_CLI_RELEASE_URL=
ARG CUA_CLI_SHA256=
ARG CUA_SPACESD_SOURCE=base
ARG CUA_SPACESD_VERSION=
ARG CUA_SPACESD_RELEASE_URL=
USER root
RUN --mount=type=bind,source=agent-home/dist,target=/dist \
    set -eu; \
    bin=/usr/local/bin/cua; \
    default_url='https://github.com/trycua/cua/releases/download/cua-sdk-v{version}/cua-cli-{version}-linux-{arch}.tar.gz'; \
    CUA_CLI_RELEASE_URL="${CUA_CLI_RELEASE_URL:-$default_url}"; \
    case "$CUA_CLI_SOURCE" in \
      local) \
        src="/dist/$TARGETARCH/cua"; \
        [ -f "$src" ] || { echo "CUA_CLI_SOURCE=local but $src is missing; run agent-home/build-cua-linux.sh or pass CUA_CLI_SOURCE=release" >&2; exit 1; }; \
        install -m 0755 "$src" "$bin" ;; \
      release) \
        case "$TARGETARCH" in amd64) arch=x64 ;; arm64) arch=arm64 ;; *) echo "no cua CLI release for $TARGETARCH" >&2; exit 1 ;; esac; \
        CUA_CLI_VERSION="${CUA_CLI_VERSION#cua-sdk-v}"; CUA_CLI_VERSION="${CUA_CLI_VERSION#v}"; \
        if [ -z "$CUA_CLI_VERSION" ] || [ "$CUA_CLI_VERSION" = latest ]; then \
          CUA_CLI_VERSION="$(curl -fsSL 'https://api.github.com/repos/trycua/cua/releases?per_page=100' \
            | python3 -c 'import json,sys; print(next(r["tag_name"][len("cua-sdk-v"):] for r in json.load(sys.stdin) if r["tag_name"].startswith("cua-sdk-v") and not r["draft"] and not r["prerelease"]))')"; \
          echo "latest cua-sdk release: $CUA_CLI_VERSION"; \
        fi; \
        url="$(echo "$CUA_CLI_RELEASE_URL" | sed "s/{version}/$CUA_CLI_VERSION/g; s/{arch}/$arch/g")"; \
        echo "fetching $url"; \
        curl -fsSL -o /tmp/cua-cli.tar.gz "$url"; \
        if [ -n "$CUA_CLI_SHA256" ]; then echo "$CUA_CLI_SHA256  /tmp/cua-cli.tar.gz" | sha256sum -c -; fi; \
        tar -xzf /tmp/cua-cli.tar.gz -C /usr/local/bin cua; \
        rm /tmp/cua-cli.tar.gz; \
        chmod 0755 "$bin"; \
        echo "$CUA_CLI_VERSION" >/tmp/cua-cli-version ;; \
      none) echo "building without the cua CLI" ;; \
      *) echo "unknown CUA_CLI_SOURCE=$CUA_CLI_SOURCE" >&2; exit 1 ;; \
    esac; \
    if [ -x "$bin" ]; then ldd "$bin" | grep 'not found' && exit 1 || true; "$bin" --version; fi; \
    mkdir -p /etc/cua-image; \
    echo "$CUA_CLI_SOURCE" >/etc/cua-image/cua-cli-source; \
    if [ -f /tmp/cua-cli-version ]; then mv /tmp/cua-cli-version /etc/cua-image/cua-cli-release; fi

# cua-spacesd, when the base image's does not serve the spawn channel.
RUN --mount=type=bind,source=agent-home/dist,target=/dist \
    set -eu; \
    bin=/usr/local/bin/cua-spacesd; \
    default_url='https://github.com/trycua/cua/releases/download/cua-spacesd-v{version}/cua-spacesd-linux-{arch}.tar.gz'; \
    CUA_SPACESD_RELEASE_URL="${CUA_SPACESD_RELEASE_URL:-$default_url}"; \
    case "$CUA_SPACESD_SOURCE" in \
      base) exit 0 ;; \
      local) \
        src="/dist/$TARGETARCH/cua-spacesd"; \
        [ -f "$src" ] || { echo "CUA_SPACESD_SOURCE=local but $src is missing" >&2; exit 1; }; \
        install -m 0755 "$src" "$bin"; echo local >/etc/cua-image/spacesd-source ;; \
      release) \
        case "$TARGETARCH" in amd64) arch=x86_64 ;; arm64) arch=aarch64 ;; *) arch="$TARGETARCH" ;; esac; \
        CUA_SPACESD_VERSION="${CUA_SPACESD_VERSION#cua-spacesd-v}"; CUA_SPACESD_VERSION="${CUA_SPACESD_VERSION#v}"; \
        [ -n "$CUA_SPACESD_VERSION" ] || { echo "CUA_SPACESD_SOURCE=release needs CUA_SPACESD_VERSION" >&2; exit 1; }; \
        url="$(echo "$CUA_SPACESD_RELEASE_URL" | sed "s/{version}/$CUA_SPACESD_VERSION/g; s/{arch}/$arch/g")"; \
        echo "fetching $url"; \
        curl -fsSL -o /tmp/cua-spacesd.tar.gz "$url"; \
        sum="$(curl -fsSL "$url.sha256" | cut -d' ' -f1)"; \
        echo "$sum  /tmp/cua-spacesd.tar.gz" | sha256sum -c -; \
        tar -xzf /tmp/cua-spacesd.tar.gz -C /usr/local/bin cua-spacesd; rm /tmp/cua-spacesd.tar.gz; \
        chmod 0755 "$bin"; echo release >/etc/cua-image/spacesd-source; echo "$CUA_SPACESD_VERSION" >/etc/cua-image/spacesd-release ;; \
      *) echo "unknown CUA_SPACESD_SOURCE=$CUA_SPACESD_SOURCE" >&2; exit 1 ;; \
    esac; \
    ldd "$bin" | grep 'not found' && exit 1 || true; "$bin" --version
