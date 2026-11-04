FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      libguestfs-tools linux-image-generic qemu-utils ca-certificates curl unzip \
    && rm -rf /var/lib/apt/lists/*

# CUA-1948: the compromised eMudhra intermediates. Ubuntu's ca-certificates >= 20260816ubuntu0.<release>.1 drops the
# roots they chain to; Debian has not shipped that yet, so disable both roots explicitly (a no-op once they are gone).
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && sed -i -E 's#^(mozilla/(emSign_ECC_Root_CA_-_G3|Comodo_AAA_Services_root)\.crt)$#!\1#' /etc/ca-certificates.conf \
 && update-ca-certificates \
 && rm -rf /var/lib/apt/lists/*

ENV LIBGUESTFS_BACKEND=direct
WORKDIR /work
