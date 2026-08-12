#!/bin/sh
# Keep Nix's glibc-bearing library path out of the prebuilt Electron child.
unset LD_LIBRARY_PATH NIX_LD NIX_LD_LIBRARY_PATH
export ACCESSIBILITY_ENABLED=1
export NO_AT_BRIDGE=0
exec "$(dirname "$0")/CuaTestHarness.Electron.bin" "$@"
