#!/usr/bin/env bash
# Make /dev/kvm usable by the runner user on a GitHub-hosted Ubuntu runner
# (they expose it, root:kvm 0660, but the runner user is not in the group).
#
#   enable-kvm.sh [--require]
#
# Uses the documented udev rule rather than a one-off chmod: the rule is
# static, so installing QEMU (which re-applies the kvm udev rules) cannot put
# /dev/kvm back to 0660 afterwards. Without KVM, QEMU falls back to TCG
# software emulation, which boots a desktop image several times slower
# (900-1300+ s on a hosted runner), so a lane that needs hardware
# acceleration passes --require and fails here, loudly, instead of timing
# out half an hour later. Off Linux this is a no-op.
set -euo pipefail
REQUIRE=0
for a in "$@"; do
    case "$a" in
        --require) REQUIRE=1 ;;
        *) echo "unknown option $a" >&2; exit 2 ;;
    esac
done
[ "$(uname -s)" = Linux ] || exit 0

fail() {
    echo "::error::$1"
    echo "$1" >&2
    exit 1
}

if [ ! -e /dev/kvm ]; then
    msg="no /dev/kvm on this runner: QEMU would run under TCG (several times slower)"
    [ "$REQUIRE" = 1 ] && fail "$msg"
    echo "::warning::$msg"
    exit 0
fi

if [ ! -w /dev/kvm ]; then
    echo 'KERNEL=="kvm", GROUP="kvm", MODE="0666", OPTIONS+="static_node=kvm"' \
        | sudo tee /etc/udev/rules.d/99-kvm4all.rules >/dev/null
    sudo udevadm control --reload-rules
    sudo udevadm trigger --name-match=kvm
    sudo udevadm settle --timeout=10 || true
fi
# The rule is the supported route; chmod only covers a host without udev.
[ -w /dev/kvm ] || sudo chmod 666 /dev/kvm || true

ls -l /dev/kvm
if [ ! -w /dev/kvm ]; then
    msg="/dev/kvm exists but is not writable by $(id -un) after the udev rule"
    [ "$REQUIRE" = 1 ] && fail "$msg"
    echo "::warning::$msg"
    exit 0
fi
echo "KVM enabled for $(id -un)"
