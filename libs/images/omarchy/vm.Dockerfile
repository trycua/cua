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
# mkinitcpio-cua.conf and pacman's kernel hooks are allowed to fail. Limine's
# "FAT32 boot partition not found" hook error in the build log is therefore
# expected and harmless (it is in green runs too).
# The VM layer installs from the same dated Arch archive snapshot as the rootfs
# (/etc/cua-image/pacman-mirrorlist.build). mirror.omarchy.org keeps no old
# package versions, and a rolling `pacman -Syu` upgrades hyprland past the exact
# version cua-hyprland-plugin pins ("installing hyprland (0.57.0-1) breaks
# dependency 'hyprland=0.56.2-4'", failing every build from 2026-10-31). Retries
# refresh the snapshot database with -Sy and never upgrade the system.
COPY omarchy/files/mkinitcpio-cua.conf /etc/cua-image/mkinitcpio-cua.conf
RUN set -eu; \
    test -s /etc/cua-image/pacman-mirrorlist.build || { echo "ERROR: /etc/cua-image/pacman-mirrorlist.build is missing: rebuild the omarchy rootfs image from this branch (the VM layer must install from the same Arch snapshot as the rootfs)." >&2; exit 1; }; \
    cp /etc/pacman.d/mirrorlist /tmp/mirrorlist.final; \
    cp /etc/cua-image/pacman-mirrorlist.build /etc/pacman.d/mirrorlist; \
    pacman -Q | sort >/tmp/rootfs-packages; \
    pkgs="linux mkinitcpio cloud-init cloud-guest-utils openssh qemu-guest-agent e2fsprogs dosfstools nftables"; \
    for attempt in 1 2 3 4; do \
      if [ "$attempt" = 1 ]; then pacman -S --noconfirm --needed $pkgs && break; \
      else pacman -Sy --noconfirm --needed $pkgs && break; fi; \
      [ "$attempt" = 4 ] && exit 1; echo "pacman attempt $attempt failed; refreshing the database and retrying" >&2; sleep 30; done; \
    kver="$(ls /usr/lib/modules | sort -V | tail -1)"; \
    install -m 0644 "/usr/lib/modules/$kver/vmlinuz" /boot/vmlinuz-linux; \
    mkinitcpio -c /etc/cua-image/mkinitcpio-cua.conf -k "$kver" -g /boot/initramfs-cua.img; \
    ln -sf vmlinuz-linux /boot/vmlinuz; \
    ln -sf initramfs-cua.img /boot/initrd.img; \
    test -s /boot/initramfs-cua.img; \
    rm -f /boot/initramfs-linux*.img; \
    pacman -Q | sort | comm -13 /tmp/rootfs-packages - | cut -d' ' -f1 >/etc/cua-image/variant-packages; \
    rm -f /tmp/rootfs-packages; \
    pacman -Q hyprland cua-hyprland-plugin >/dev/null || { echo "ERROR: hyprland and cua-hyprland-plugin are not both installed after the VM layer; the plugin pins an exact hyprland version." >&2; exit 1; }; \
    cp /tmp/mirrorlist.final /etc/pacman.d/mirrorlist; rm -f /tmp/mirrorlist.final; \
    yes | pacman -Scc >/dev/null

COPY common/vm/files/fstab /etc/fstab
COPY common/vm/files/99-cua-cloud.cfg /etc/cloud/cloud.cfg.d/99-cua-cloud.cfg
# CVE-2026-98432 interim guard (IPv4 options dropped before TCP); remove once the
# snapshot's kernel is at or above the fixed version for its series.
COPY common/vm/files/cua-ipopts/cua-ipopts-guard /opt/cua/bin/cua-ipopts-guard
COPY common/vm/files/cua-ipopts/cua-ipopts.nft /etc/cua-image/cua-ipopts.nft
COPY common/vm/files/cua-ipopts/cua-ipopts.service /etc/systemd/system/cua-ipopts.service
COPY common/vm/files/cua-ipopts/90-cua-ipopts.conf /etc/sysctl.d/90-cua-ipopts.conf
RUN chmod 0755 /opt/cua/bin/cua-ipopts-guard && systemctl enable cua-ipopts.service
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
