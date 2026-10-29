# A minimal Space for the opt-in crash tests: Ubuntu with a cua-spacesd
# build (no desktop, no driver). Build it from a directory holding the
# binary:
#   cp <cua-spacesd> ctx/ && docker build -t cua-pi-space:dev -f Dockerfile ctx
FROM ubuntu:24.04
RUN apt-get update \
 && apt-get install -y --no-install-recommends libx11-6 libxi6 libxext6 ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && mkdir -p /work /var/lib/cua-spacesd

# CUA-1948: the compromised eMudhra intermediates. Ubuntu's ca-certificates >= 20260816ubuntu0.<release>.1 drops the
# roots they chain to; Debian has not shipped that yet, so disable both roots explicitly (a no-op once they are gone).
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && sed -i -E 's#^(mozilla/(emSign_ECC_Root_CA_-_G3|Comodo_AAA_Services_root)\.crt)$#!\1#' /etc/ca-certificates.conf \
 && update-ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY cua-spacesd /usr/local/bin/cua-spacesd
ENV CUA_ENV_DATA_DIR=/var/lib/cua-spacesd
EXPOSE 3211
ENTRYPOINT ["/usr/local/bin/cua-spacesd", "serve", "--listen", "0.0.0.0:3211", "--no-desktop", "--no-driver", "--no-access-log", "--no-mcp"]
