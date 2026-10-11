#!/bin/zsh
set -euo pipefail

# Install the protected helper into a disposable macOS benchmark seed.
# Run only as root, with the existing `lume` control account and `cdb-agent`
# standard account already provisioned.

if [[ ${EUID} -ne 0 ]]; then
  print -u2 "install-cdb-helper: root is required"
  exit 64
fi
if [[ $# -ne 2 || $1 != /* || ! -f $1 || -L $1 || $2 != /* || ! -f $2 || -L $2 ]]; then
  print -u2 "usage: install-cdb-helper.sh /absolute/path/to/cdb-helper /absolute/path/to/cdb-driver-mediator"
  exit 64
fi
if ! /usr/bin/id -u lume >/dev/null 2>&1 || ! /usr/bin/id -u cdb-agent >/dev/null 2>&1; then
  print -u2 "install-cdb-helper: required seed accounts are absent"
  exit 65
fi

helper_source=$1
mediator_source=$2
helper_target=/usr/local/libexec/cdb-helper
mediator_target=/usr/local/libexec/cdb-driver-mediator
sudoers_target=/etc/sudoers.d/cdb-helper
sshd_target=/etc/ssh/sshd_config.d/00-cdb-benchmark.conf
pf_target=/etc/pf.conf
scratch=$(/usr/bin/mktemp -d /private/tmp/cdb-helper-install.XXXXXX)
trap '/usr/bin/find "$scratch" -depth -delete 2>/dev/null || true' EXIT

/usr/bin/install -d -o root -g wheel -m 0755 /usr/local/libexec /etc/sudoers.d /etc/ssh/sshd_config.d
/usr/bin/install -d -o lume -g staff -m 0700 /Users/lume/Library/Caches/cdb-inbox
/usr/bin/install -d -o root -g wheel -m 0755 /Users/Shared/cdb-attempts
/usr/bin/install -o root -g wheel -m 0555 "$helper_source" "$helper_target"
/usr/bin/install -o root -g wheel -m 0555 "$mediator_source" "$mediator_target"

/usr/bin/printf '%s\n' \
  'lume ALL=(root) NOPASSWD: /usr/local/libexec/cdb-helper *' \
  > "$scratch/sudoers"
/usr/sbin/visudo -cf "$scratch/sudoers" >/dev/null
/usr/bin/install -o root -g wheel -m 0440 "$scratch/sudoers" "$sudoers_target"

/usr/bin/printf '%s\n' \
  'PasswordAuthentication no' \
  'KbdInteractiveAuthentication no' \
  > "$scratch/sshd"
/usr/bin/install -o root -g wheel -m 0644 "$scratch/sshd" "$sshd_target"
/usr/sbin/sshd -T -f /etc/ssh/sshd_config > "$scratch/sshd-effective"
/usr/bin/grep -Fqx 'passwordauthentication no' "$scratch/sshd-effective"
/usr/bin/grep -Fqx 'kbdinteractiveauthentication no' "$scratch/sshd-effective"

/usr/bin/awk '
  $0 == "anchor \"com.trycua.cdb/*\"" { next }
  !inserted && $0 ~ /^[[:space:]]*anchor[[:space:]]/ {
    print "anchor \"com.trycua.cdb/*\""
    inserted = 1
  }
  { print }
  END {
    if (!inserted) print "anchor \"com.trycua.cdb/*\""
  }
' "$pf_target" > "$scratch/pf.conf"
/sbin/pfctl -nf "$scratch/pf.conf" >/dev/null
/usr/bin/install -o root -g wheel -m 0644 "$scratch/pf.conf" "$pf_target"
/sbin/pfctl -f "$pf_target" >/dev/null
/sbin/pfctl -sr | /usr/bin/grep -Fq 'anchor "com.trycua.cdb/*"'

/bin/launchctl kickstart -k system/com.openssh.sshd >/dev/null 2>&1 || true
/usr/bin/shasum -a 256 "$helper_target" "$mediator_target" "$sudoers_target"
