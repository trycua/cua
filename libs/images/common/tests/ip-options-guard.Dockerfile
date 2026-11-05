# syntax=docker/dockerfile:1.7
# Throwaway systemd-as-PID-1 Ubuntu container for test-ip-options-guard.sh: the
# CVE-2026-98432 guard installed with the same COPY and `systemctl enable` lines
# as common/vm/Dockerfile (a static check in test-ip-options-guard-static.sh
# keeps the two in step). Build context: libs/images.
FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      systemd systemd-sysv dbus kmod iproute2 nftables iptables iputils-ping \
      python3 python3-scapy procps ca-certificates \
 && sed -i -E 's#^(mozilla/(emSign_ECC_Root_CA_-_G3|Comodo_AAA_Services_root)\.crt)$#!\1#' /etc/ca-certificates.conf \
 && update-ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY common/vm/files/cua-ipopts/cua-ipopts-guard /opt/cua/bin/cua-ipopts-guard
COPY common/vm/files/cua-ipopts/cua-ipopts.nft /etc/cua-image/cua-ipopts.nft
COPY common/vm/files/cua-ipopts/cua-ipopts.service /etc/systemd/system/cua-ipopts.service
COPY common/vm/files/cua-ipopts/90-cua-ipopts.conf /etc/sysctl.d/90-cua-ipopts.conf
COPY common/tests/ip-options-guard-guest.sh /opt/cua/tests/ip-options-guard-guest.sh
RUN chmod 0755 /opt/cua/bin/cua-ipopts-guard /opt/cua/tests/ip-options-guard-guest.sh \
 && systemctl enable cua-ipopts.service
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
