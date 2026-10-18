# syntax=docker/dockerfile:1.7
# Plain desktop image: Ubuntu 24.04 + XFCE on Xvnc :1 (RFB 5901). Nothing
# else -- no cua-spacesd, no computer-server, no cua-driver, no MCP, no
# noVNC. The e2e suite uses it to prove sandboxes are daemon-agnostic: the SDK
# must create it, wait for readiness (tcp:5901) and drive it over VNC only.
#
# Build context: libs/images. Same two outputs as every image here (see
# ../../build.sh): `docker-` rootfs for docker/gVisor and a containerDisk.
ARG UBUNTU_VERSION=24.04
FROM ubuntu:${UBUNTU_VERSION}
ENV DEBIAN_FRONTEND=noninteractive LANG=C.UTF-8 LC_ALL=C.UTF-8 TZ=Etc/UTC
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates sudo tzdata procps iproute2 less \
      supervisor dbus dbus-x11 \
      xfce4 xfce4-terminal xfwm4 xfdesktop4 xfce4-panel xfce4-settings thunar \
      greybird-gtk-theme adwaita-icon-theme fonts-dejavu-core \
      tigervnc-standalone-server tigervnc-tools at-spi2-core x11-utils \
 && rm -rf /var/lib/apt/lists/*

COPY common/desktop/ /opt/cua/desktop/
COPY plain/ubuntu-xfce-vnc/files/supervisord.conf /etc/supervisor/supervisord.conf
COPY common/desktop/systemd/cua-desktop.service /usr/lib/systemd/system/cua-desktop.service
RUN /opt/cua/desktop/install-desktop-user.sh cua 1000 \
 && ln -s /opt/cua/desktop/desktop-env.sh /usr/local/bin/desktop-env \
 && mkdir -p /etc/cua-image \
 && echo cua-desktop.service >/etc/cua-image/vm-units \
 && echo container >/etc/cua-image/variant

ENV HOME=/root DISPLAY=:1
EXPOSE 5901
HEALTHCHECK --interval=10s --timeout=5s --start-period=30s --retries=12 \
  CMD ["/opt/cua/desktop/healthcheck.sh"]
CMD ["/opt/cua/desktop/entrypoint.sh"]
