# libguestfs helper for scripts/bench-images: guestfish file operations and
# tar-out on upstream qcow2 disks. Runs natively on either arch (it never
# executes guest binaries, so an amd64 disk is customized the same way on an
# arm64 host), without --privileged; /dev/kvm is used when passed in.
FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      libguestfs-tools linux-image-generic qemu-utils ca-certificates curl unzip \
      python3 zstd pigz file \
    && rm -rf /var/lib/apt/lists/*
ENV LIBGUESTFS_BACKEND=direct
WORKDIR /work
