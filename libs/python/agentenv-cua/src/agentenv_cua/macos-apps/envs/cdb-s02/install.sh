#!/bin/sh
# Run once the app's downloads are in, as the session user: unpack the runtimes they brought, Electron, and Node and
# Chrome for Testing where the task needs them, in place of their archives. ditto keeps what a macOS bundle needs
# (symlinks, execute bits) that unzip can lose.
set -e
for zip in runtime/electron-*.zip; do
    [ -e "$zip" ] || continue
    ditto -x -k "$zip" runtime/electron
    rm "$zip"
done
for tarball in runtime/node-*.tar.xz; do
    [ -e "$tarball" ] || continue
    mkdir -p runtime/node
    tar -xJf "$tarball" -C runtime/node --strip-components=1
    rm "$tarball"
done
test -x runtime/electron/Electron.app/Contents/MacOS/Electron
