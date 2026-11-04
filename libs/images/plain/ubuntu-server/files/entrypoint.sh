#!/usr/bin/env bash
# Container entrypoint for plain/ubuntu-server: host keys, optional creds, sshd.
set -euo pipefail
ssh-keygen -A >/dev/null
install -d -m 0755 /run/sshd
if [ -n "${SSH_AUTHORIZED_KEYS:-}" ]; then
    install -d -m 0700 -o cua -g cua /home/cua/.ssh
    printf '%s\n' "$SSH_AUTHORIZED_KEYS" >/home/cua/.ssh/authorized_keys
    chown cua:cua /home/cua/.ssh/authorized_keys; chmod 600 /home/cua/.ssh/authorized_keys
fi
if [ -n "${SSH_PASSWORD:-}" ]; then
    echo "cua:${SSH_PASSWORD}" | chpasswd
    sed -i 's/^PasswordAuthentication no/PasswordAuthentication yes/' /etc/ssh/sshd_config.d/10-cua.conf
fi
unset SSH_PASSWORD
exec /usr/sbin/sshd -D -e
