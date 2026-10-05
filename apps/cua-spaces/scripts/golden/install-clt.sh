#!/bin/bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# install-clt.sh — install the Xcode Command Line Tools with no GUI dialog.
#
# Unity's package manager shells out to `git`, and macOS ships only *stubs* for
# git/python3 until the Command Line Tools are installed. Invoking a stub pops
# the "The 'git' command requires the command line developer tools" dialog —
# which, inside a Space, is a modal nothing can dismiss, so package resolution
# hangs forever.
#
# `xcode-select --install` is itself a GUI installer, so it cannot be used here.
# The headless route is the documented softwareupdate trick: drop the sentinel
# file that makes `softwareupdate` list the CLT package, then install it by
# label.
set -uo pipefail

say() { printf '\033[1;36m==> %s\033[0m\n' "$*"; }

# This runs over SSH with no tty, so `sudo` cannot prompt. Feed it the account
# password on stdin (override with CUA_SUDO_PW). `-p ''` suppresses the prompt
# text so it never lands in the log.
sudo_run() { printf '%s\n' "${CUA_SUDO_PW:-lume}" | sudo -S -p '' "$@"; }

if /usr/bin/xcode-select -p >/dev/null 2>&1 && /usr/bin/git --version >/dev/null 2>&1; then
  say "Command Line Tools already present: $(/usr/bin/git --version)"
  exit 0
fi

SENTINEL="/tmp/.com.apple.dt.CommandLineTools.installondemand.in-progress"
say "Making softwareupdate offer the Command Line Tools"
sudo_run /usr/bin/touch "$SENTINEL"

# The label varies by OS release (e.g. "Command Line Tools for Xcode-16.2"),
# so discover it rather than hardcoding, and take the highest version offered.
LABEL=$(softwareupdate -l 2>/dev/null \
  | grep -B1 -E 'Command Line Tools' \
  | awk -F'Label: ' '/Label: /{print $2}' \
  | sort -V | tail -1)

if [ -z "${LABEL:-}" ]; then
  sudo_run /bin/rm -f "$SENTINEL"
  echo "No Command Line Tools package offered by softwareupdate." >&2
  echo "This usually means the OS already considers them installed but the" >&2
  echo "receipt is broken; try: sudo rm -rf /Library/Developer/CommandLineTools" >&2
  exit 1
fi

say "Installing: $LABEL"
sudo_run softwareupdate -i "$LABEL" --verbose
sudo_run /bin/rm -f "$SENTINEL"

# Point xcode-select at the tools so `git` resolves without a stub.
if [ -d /Library/Developer/CommandLineTools ]; then
  sudo_run /usr/bin/xcode-select -s /Library/Developer/CommandLineTools
fi

say "Verifying (these must NOT open a dialog)"
/usr/bin/git --version || { echo "git still unavailable" >&2; exit 1; }
/usr/bin/python3 --version || { echo "python3 still unavailable" >&2; exit 1; }
say "Command Line Tools ready"
