#!/usr/bin/env bash
set -euo pipefail

[[ ${EUID} -eq 0 ]] || {
  echo 'Run this script with sudo.' >&2
  exit 1
}
[[ $(uname -m) == aarch64 ]] || {
  echo 'This setup is only for an ARM64 Linux guest.' >&2
  exit 1
}

install -d -m0755 /mnt/rosetta
mountpoint -q /mnt/rosetta || mount -t virtiofs rosetta /mnt/rosetta
[[ -x /mnt/rosetta/rosetta ]] || {
  echo 'The Lume Rosetta share is unavailable. Install Rosetta on the macOS host.' >&2
  exit 1
}

grep -q '^[[:space:]]*rosetta[[:space:]]\+/mnt/rosetta[[:space:]]\+virtiofs' /etc/fstab || \
  printf '%s\n' 'rosetta /mnt/rosetta virtiofs ro,nofail 0 0' >>/etc/fstab

install -d -m0755 /etc/binfmt.d
cat >/etc/binfmt.d/rosetta.conf <<'BINFMT'
:rosetta:M::\x7fELF\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x00\x3e\x00:\xff\xff\xff\xff\xff\xfe\xfe\x00\xff\xff\xff\xff\xff\xff\xff\xff\xfe\xff\xff\xff:/mnt/rosetta/rosetta:OCF
BINFMT

systemctl restart systemd-binfmt
cat /proc/sys/fs/binfmt_misc/rosetta
