# syntax=docker/dockerfile:1.7
# Turn the omarchy guest tree (Arch) into a bootable VM root: the Arch form of
# common/vm/Dockerfile, which is apt-based. Adds a kernel + initramfs, cloud-init,
# sshd and qemu-guest-agent, NetworkManager DHCP for KubeVirt, and enables the
# units in /etc/cua-image/vm-units (sddm autologin into the Omarchy session,
# cua-spacesd). The bootloader is written by common/disk-builder/make-disk.sh,
# which expects /boot/vmlinuz, /boot/initrd.img and root=LABEL=cloudimg-rootfs.
#
#   docker buildx build -f omarchy/vm.Dockerfile --build-arg BASE_IMAGE=<rootfs ref> ...
ARG BASE_IMAGE
FROM ${BASE_IMAGE}
USER root

# Omarchy's mkinitcpio drop-ins (plymouth, limine, btrfs) target its ISO
# install; this disk is ext4 behind GRUB, so the initramfs is built from
# mkinitcpio-cua.conf and pacman's kernel hooks are allowed to fail.
COPY omarchy/files/mkinitcpio-cua.conf /etc/cua-image/mkinitcpio-cua.conf
RUN set -eu; \
    pacman -Q | sort >/tmp/rootfs-packages; \
    for attempt in 1 2 3; do pacman -S --noconfirm --needed \
        linux mkinitcpio cloud-init cloud-guest-utils openssh qemu-guest-agent \
        e2fsprogs dosfstools && break; \
      [ "$attempt" = 3 ] && exit 1; echo "pacman attempt $attempt failed; retrying" >&2; sleep 5; done; \
    kver="$(ls /usr/lib/modules | sort -V | tail -1)"; \
    install -m 0644 "/usr/lib/modules/$kver/vmlinuz" /boot/vmlinuz-linux; \
    mkinitcpio -c /etc/cua-image/mkinitcpio-cua.conf -k "$kver" -g /boot/initramfs-cua.img; \
    ln -sf vmlinuz-linux /boot/vmlinuz; \
    ln -sf initramfs-cua.img /boot/initrd.img; \
    test -s /boot/initramfs-cua.img; \
    rm -f /boot/initramfs-linux*.img; \
    pacman -Q | sort | comm -13 /tmp/rootfs-packages - | cut -d' ' -f1 >/etc/cua-image/variant-packages; \
    rm -f /tmp/rootfs-packages; \
    yes | pacman -Scc >/dev/null

COPY common/vm/files/fstab /etc/fstab
COPY common/vm/files/99-cua-cloud.cfg /etc/cloud/cloud.cfg.d/99-cua-cloud.cfg
COPY omarchy/files/kubevirt.nmconnection /etc/NetworkManager/system-connections/kubevirt.nmconnection
RUN chmod 600 /etc/NetworkManager/system-connections/kubevirt.nmconnection \
 && rm -f /etc/ssh/ssh_host_* /var/lib/dbus/machine-id

RUN set -eu; \
    systemctl enable NetworkManager.service systemd-resolved.service sshd.service qemu-guest-agent.service \
      cloud-init-main.service cloud-init-local.service cloud-init-network.service cloud-config.service cloud-final.service 2>/dev/null \
      || systemctl enable NetworkManager.service systemd-resolved.service sshd.service qemu-guest-agent.service cloud-init.service cloud-config.service cloud-final.service; \
    systemctl mask NetworkManager-wait-online.service; \
    systemctl enable systemd-timesyncd.service; \
    # /tmp on the root disk, as on the other images (Arch mounts a RAM tmpfs).
    systemctl mask tmp.mount; \
    grep -v '^[[:space:]]*\(#\|$\)' /etc/cua-image/vm-units | xargs -r systemctl enable; \
    systemctl set-default graphical.target; \
    echo vm >/etc/cua-image/variant; \
    /opt/cua/bin/cua-image-manifest set-variant --in /etc/cua-image/manifest.json \
      --image-json /etc/cua-image/image.json --variant containerdisk
