#!/bin/bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# seed-python-for-unity-mcp.sh — make MCP for Unity's dependency check pass.
#
# The editor package's "MCP Setup" window requires **Python 3.10+**, but macOS
# only ships /usr/bin/python3 = 3.9.6, so it reports:
#   Python  Not Found  — "Python not found in PATH or standard locations"
#   Missing dependencies. MCP for Unity requires all dependencies to function.
#
# GUI apps launched via `open` inherit PATH=/usr/bin:/bin:/usr/sbin:/sbin, so a
# shell-profile PATH edit does nothing for them. The package's macOS detector
# searches, in order: ~/.pyenv/shims, /opt/homebrew/bin, /usr/local/bin,
# /usr/bin, /bin, ~/.local/bin — so linking a modern interpreter into
# /usr/local/bin gets picked up ahead of the 3.9.6 system one.
#
# uv already manages a modern CPython in this image, so reuse it rather than
# installing a second Python.
set -u

DEST=/usr/local/bin
# Over SSH there is no tty for sudo to prompt on, so feed the password in.
sudo_run() { printf '%s\n' "${CUA_SUDO_PW:-lume}" | sudo -S -p '' "$@"; }
SUDO=""
[ -w "$DEST" ] || SUDO="sudo_run"

# Newest uv-managed CPython >= 3.10 (version-sorted so 3.14 wins over 3.10).
PY=$(ls -d "$HOME"/.local/share/uv/python/cpython-3.1[0-9]*/bin/python3.1[0-9] 2>/dev/null | sort -V | tail -1)
if [ -z "${PY:-}" ]; then
  echo "no uv-managed python >=3.10 found; run: uv python install 3.13" >&2
  exit 1
fi
echo "using $PY ($("$PY" --version 2>&1))"

$SUDO mkdir -p "$DEST"
# python3 + python so either probe name resolves; uv/uvx too, so the package
# finds them without depending on ~/.local/bin being on a GUI PATH.
for name in python3 python; do
  $SUDO ln -sf "$PY" "$DEST/$name"
done
for tool in uv uvx; do
  [ -x "$HOME/.local/bin/$tool" ] && $SUDO ln -sf "$HOME/.local/bin/$tool" "$DEST/$tool"
done

echo "--- what the detector will now find ---"
for n in python3 python uv uvx; do
  printf '%-8s %s\n' "$n" "$("$DEST/$n" --version 2>&1 | head -1)"
done
