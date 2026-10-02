# syntax=docker/dockerfile:1.7
# Tool image for make-disk.sh: turns an exported rootfs tar into a bootable
# GPT qcow2 without loop devices, nbd, libguestfs or --privileged. Build and
# run it for the TARGET architecture (the GRUB platform packages only exist
# for their own arch; on an arm64 host, amd64 runs under Rosetta/qemu-user).
FROM ubuntu:24.04
ARG TARGETARCH
ENV DEBIAN_FRONTEND=noninteractive
RUN set -eux; \
    case "$TARGETARCH" in \
      amd64) grub="grub-pc-bin grub-efi-amd64-bin" ;; \
      arm64) grub="grub-efi-arm64-bin" ;; \
      *) echo "unsupported arch $TARGETARCH"; exit 1 ;; \
    esac; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
      e2fsprogs dosfstools mtools fdisk qemu-utils uuid-runtime \
      grub-common grub2-common $grub; \
    rm -rf /var/lib/apt/lists/*
COPY make-disk.sh /usr/local/bin/make-disk
ENTRYPOINT ["/usr/local/bin/make-disk"]
