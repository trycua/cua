FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      libguestfs-tools linux-image-generic qemu-utils ca-certificates curl unzip \
    && rm -rf /var/lib/apt/lists/*
ENV LIBGUESTFS_BACKEND=direct
WORKDIR /work
