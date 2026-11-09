#!/usr/bin/env bash
set -euo pipefail

# This script erases the selected disk. It is intended only for a fresh Lume VM.
disk=${1:-/dev/vda}
target=/mnt
timezone=${TIMEZONE:-UTC}

if [[ ${disk} != /dev/vda ]]; then
  printf 'Refusing unexpected disk %s. Pass /dev/vda for the Lume VM.\n' "${disk}" >&2
  exit 1
fi

[[ -e /usr/share/zoneinfo/${timezone} ]] || {
  printf 'Unknown timezone: %s\n' "${timezone}" >&2
  exit 1
}

printf 'Installing Arch Linux ARM to %s. ALL DATA ON THIS DISK WILL BE LOST.\n' "${disk}"
read -r -p 'Type ERASE to continue: ' confirmation
[[ ${confirmation} == ERASE ]] || exit 1

sgdisk --zap-all "${disk}"
sgdisk -n 1:1MiB:+1GiB -t 1:ef00 -c 1:EFI "${disk}"
sgdisk -n 2:0:0 -t 2:8300 -c 2:ROOT "${disk}"
partx -u "${disk}"
udevadm settle

mkfs.fat -F 32 -n EFI "${disk}1"
mkfs.btrfs -f -L ARCH "${disk}2"
mount "${disk}2" "${target}"
mkdir -p "${target}/boot"
mount "${disk}1" "${target}/boot"

# Installing the ARM keyring here avoids a circular key-trust bootstrap later.
pacstrap -K "${target}" \
  base base-devel linux-aarch64 linux-firmware archlinuxarm-keyring \
  btrfs-progs dosfstools efibootmgr git networkmanager openssh sudo

genfstab -U "${target}" >>"${target}/etc/fstab"

arch-chroot "${target}" /bin/bash -s -- "${timezone}" <<'CHROOT'
set -euo pipefail
timezone=$1

ln -sf "/usr/share/zoneinfo/${timezone}" /etc/localtime
hwclock --systohc
sed -i 's/^#en_US.UTF-8 UTF-8/en_US.UTF-8 UTF-8/' /etc/locale.gen
locale-gen
printf 'LANG=en_US.UTF-8\n' >/etc/locale.conf
printf 'omarchy-arm64\n' >/etc/hostname
cat >/etc/hosts <<'HOSTS'
127.0.0.1 localhost
::1 localhost
127.0.1.1 omarchy-arm64.localdomain omarchy-arm64
HOSTS

useradd -m -G wheel -s /bin/bash omarchy
printf '%%wheel ALL=(ALL:ALL) NOPASSWD: ALL\n' >/etc/sudoers.d/10-wheel
chmod 440 /etc/sudoers.d/10-wheel

systemctl enable NetworkManager sshd systemd-timesyncd
sed -i 's/^#PermitRootLogin.*/PermitRootLogin prohibit-password/' /etc/ssh/sshd_config

bootctl install
cat >/boot/loader/loader.conf <<'LOADER'
default arch.conf
timeout 2
console-mode keep
editor no
LOADER

root_uuid=$(blkid -s UUID -o value /dev/vda2)
cat >/boot/loader/entries/arch.conf <<ENTRY
title Arch Linux ARM64
linux /Image
initrd /initramfs-linux.img
options root=UUID=${root_uuid} rw rootwait quiet
ENTRY
CHROOT

printf '\nBase installation complete. Set the omarchy user password next:\n'
printf '  arch-chroot /mnt passwd omarchy\n'
