# syntax=docker/dockerfile:1.7
# Turn any Ubuntu 24.04 rootfs image from libs/images into a bootable VM root.
#
# The container variant is the single source of truth for what is installed;
# this layer only adds what a VM needs and a container must not have: a
# kernel + initramfs, systemd as init, udev, cloud-init, netplan (with
# KubeVirt's static DHCP match), sshd, and qemu-guest-agent. The bootloader is
# NOT installed here: make-disk.sh writes GRUB straight into the disk image,
# so this rootfs never runs grub-probe against a device that doesn't exist.
#
# Each base image lists the systemd units that replace its supervisord
# programs in /etc/cua-image/vm-units (one per line); they are enabled here.
#
#   docker buildx build -f common/vm/Dockerfile --build-arg BASE_IMAGE=<rootfs ref> ...
ARG BASE_IMAGE
FROM ${BASE_IMAGE}
USER root
ENV DEBIAN_FRONTEND=noninteractive

# Same retry as the rootfs: under qemu-user (foreign arch) python postinsts
# (cloud-init) intermittently segfault; re-running the configure step passes.
RUN dpkg-query -W -f '${Package}\n' | sort >/tmp/rootfs-packages \
 && apt-get update \
 && for attempt in 1 2 3; do apt-get install -y --no-install-recommends \
      linux-image-virtual initramfs-tools \
      systemd systemd-sysv systemd-resolved systemd-timesyncd udev dbus kmod \
      netplan.io cloud-init cloud-guest-utils openssh-server qemu-guest-agent \
      iproute2 iputils-ping e2fsprogs sudo ca-certificates \
      && break; echo "apt attempt $attempt failed; reconfiguring" >&2; dpkg --configure -a || true; \
      [ "$attempt" = 3 ] && exit 1; done \
 && rm -rf /var/lib/apt/lists/* \
 && kernel="$(ls -1 /boot/vmlinuz-* | sort -V | tail -1)" \
 && ln -sf "$(basename "$kernel")" /boot/vmlinuz \
 && ln -sf "initrd.img-${kernel#/boot/vmlinuz-}" /boot/initrd.img \
 && test -s "/boot/initrd.img-${kernel#/boot/vmlinuz-}" \
 && mkdir -p /etc/cua-image \
 && dpkg-query -W -f '${Package}\n' | sort | comm -13 /tmp/rootfs-packages - >/etc/cua-image/variant-packages \
 && rm -f /tmp/rootfs-packages
# /etc/cua-image/variant-packages: what the VM layer added on top of the
# rootfs. The doctor's fidelity package hash leaves them out, so the two
# variants of one image compare equal unless shared packages differ.

COPY common/vm/files/99-kubevirt.yaml /etc/netplan/99-kubevirt.yaml
COPY common/vm/files/fstab /etc/fstab
COPY common/vm/files/99-cua-cloud.cfg /etc/cloud/cloud.cfg.d/99-cua-cloud.cfg

RUN chmod 600 /etc/netplan/99-kubevirt.yaml \
 && rm -f /etc/netplan/50-cloud-init.yaml /etc/ssh/ssh_host_* /var/lib/dbus/machine-id
# (machine-id, hostname, resolv.conf and /.dockerenv are build-time bind mounts;
# make-disk.sh fixes them up in the exported tree.)

# systemd owns the VM: enable the base services plus the image's own units, and
# keep supervisord (the container PID 1) from also starting them.
RUN systemctl enable systemd-networkd systemd-resolved ssh.socket qemu-guest-agent \
 && if systemctl list-unit-files supervisor.service >/dev/null 2>&1; then systemctl mask supervisor.service; fi \
 && if [ -f /etc/cua-image/vm-units ]; then \
      grep -v '^[[:space:]]*\(#\|$\)' /etc/cua-image/vm-units | xargs -r systemctl enable; \
    fi \
 && systemctl set-default multi-user.target

# Marker read by make-disk.sh and by the image's own scripts; images with a
# /etc/cua-image/manifest.json get its variant fields (containerdisk,
# systemd, the VM units) rewritten for the doctor.
RUN mkdir -p /etc/cua-image && echo vm >/etc/cua-image/variant \
 && if [ -x /opt/cua/bin/cua-image-manifest ] && [ -f /etc/cua-image/manifest.json ]; then \
      /opt/cua/bin/cua-image-manifest set-variant --in /etc/cua-image/manifest.json \
        --image-json /etc/cua-image/image.json --variant containerdisk; \
    fi
