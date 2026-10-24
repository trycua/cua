#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Checks where a built Cua Spaces app keeps its executables:
#
#   Contents/MacOS     only the app's own executable (CFBundleExecutable),
#                      the `cua` CLI and the microVM monitor `cua-vz`
#   Contents/Helpers   the host service, cua-spacesd
#
# Why: AppKit and LaunchServices take a process's bundle from its
# executable's path, and any executable in Contents/MacOS is the app. When
# a background process there starts AppKit (cua-spacesd does, for its cursor
# overlay), it checks in with LaunchServices under the app's bundle id and
# name, and opening Cua Spaces (Dock, Finder, `open -a`) then activates that
# windowless process instead of launching the app: no window ever appears.
# From Contents/Helpers it has no bundle and runs as an unnamed UI element.
# A new executable goes to Contents/Helpers unless it is the app itself or a
# command-line tool that never starts AppKit.
#
#   scripts/check-bundle-layout.sh <App.app>
set -euo pipefail
app="${1:?usage: check-bundle-layout.sh <App.app>}"
[ -d "$app/Contents" ] || { echo "$app is not an app bundle" >&2; exit 1; }
main="$(/usr/libexec/PlistBuddy -c "Print :CFBundleExecutable" "$app/Contents/Info.plist" 2>/dev/null || true)"
[ -n "$main" ] || { echo "$app/Contents/Info.plist has no CFBundleExecutable" >&2; exit 1; }

bad=0
for f in "$app/Contents/MacOS"/*; do
  [ -e "$f" ] || continue
  name="$(basename "$f")"
  case "$name" in
    "$main" | cua | cua-vz) ;;
    *)
      echo "$app: Contents/MacOS/$name is not the app or a command-line tool; put it in Contents/Helpers (from Contents/MacOS, a process that starts AppKit registers as the app and takes over its launch)" >&2
      bad=1
      ;;
  esac
done
if [ ! -x "$app/Contents/Helpers/cua-spacesd" ]; then
  echo "$app has no Contents/Helpers/cua-spacesd (the host service)" >&2
  bad=1
fi
exit "$bad"
