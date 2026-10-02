# syntax=docker/dockerfile:1.7
# Plain server image: Ubuntu 24.04 + OpenSSH. No display, no cua daemons.
# The e2e suite uses it to prove sandboxes are daemon-agnostic (readiness =
# tcp:22, access = SSH only).
#
# Credentials (container variant), both optional:
#   SSH_AUTHORIZED_KEYS  written to ~cua/.ssh/authorized_keys
#   SSH_PASSWORD         sets cua's password and enables password auth
# The VM variant takes keys from cloud-init user-data instead.
ARG UBUNTU_VERSION=24.04
FROM ubuntu:${UBUNTU_VERSION}
ENV DEBIAN_FRONTEND=noninteractive LANG=C.UTF-8 LC_ALL=C.UTF-8 TZ=Etc/UTC
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      openssh-server sudo ca-certificates curl procps iproute2 less python3 \
 && rm -rf /var/lib/apt/lists/* \
 && rm -f /etc/ssh/ssh_host_* \
 && (userdel -r ubuntu 2>/dev/null || true) \
 && useradd -m -u 1000 -s /bin/bash -G sudo cua \
 && passwd -l cua \
 && echo 'cua ALL=(ALL) NOPASSWD:ALL' >/etc/sudoers.d/90-cua && chmod 0440 /etc/sudoers.d/90-cua \
 && printf 'PasswordAuthentication no\nPermitRootLogin no\n' >/etc/ssh/sshd_config.d/10-cua.conf \
 && mkdir -p /etc/cua-image && echo ssh.socket >/etc/cua-image/vm-units \
 && echo container >/etc/cua-image/variant
COPY plain/ubuntu-server/files/entrypoint.sh /usr/local/sbin/entrypoint.sh
EXPOSE 22
HEALTHCHECK --interval=5s --timeout=3s --start-period=10s --retries=12 \
  CMD ["bash", "-c", "exec 3<>/dev/tcp/127.0.0.1/22"]
CMD ["/usr/local/sbin/entrypoint.sh"]
