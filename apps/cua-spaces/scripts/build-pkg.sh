#!/bin/sh
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Builds the macOS .pkg (productbuild) that installs Cua Spaces.app into
# /Applications and links /usr/local/bin/cua to the CLI bundled inside it.
# Meant for MDM; the .dmg stays the interactive download.
#
#   scripts/build-pkg.sh --app "path/Cua Spaces.app" --version 0.2.0 \
#       [--out dist/cua-spaces-0.2.0-darwin-universal.pkg] \
#       [--mode host|client] [--sign "Developer ID Installer: Name (TEAM)"] \
#       [--min-os 26.0]
#
# --min-os is the oldest macOS the package installs on (default 13.0, the
# Tauri app's; the SwiftUI app needs 26.0).
#
# --mode bakes an install mode into the package; postinstall writes it to
# /Library/Application Support/Cua/spaces-install-mode, which preselects the
# app's first-run choice. Nothing is installed on the build machine.
set -eu

here="$(cd "$(dirname "$0")/.." && pwd)"
res="$here/src-tauri/installer/macos-pkg"
app=""
version=""
out=""
mode=""
sign=""
min_os="13.0"
while [ $# -gt 0 ]; do
    case "$1" in
    --app)
        app="$2"
        shift
        ;;
    --version)
        version="$2"
        shift
        ;;
    --out)
        out="$2"
        shift
        ;;
    --mode)
        mode="$2"
        shift
        ;;
    --sign)
        sign="$2"
        shift
        ;;
    --min-os)
        min_os="$2"
        shift
        ;;
    -h | --help)
        sed -n '2,16p' "$0"
        exit 0
        ;;
    *)
        echo "unknown option: $1" >&2
        exit 2
        ;;
    esac
    shift
done
[ -d "$app" ] || {
    echo "--app must be a .app bundle" >&2
    exit 2
}
case "$version" in
[0-9]*.[0-9]*.[0-9]*) ;;
*)
    echo "--version must look like 1.2.3" >&2
    exit 2
    ;;
esac
case "$min_os" in
[0-9]*.[0-9]*) ;;
*)
    echo "--min-os must look like 26.0" >&2
    exit 2
    ;;
esac
case "$mode" in "" | host | client) ;; *)
    echo "--mode must be host or client" >&2
    exit 2
    ;;
esac
[ -n "$out" ] || out="$PWD/cua-spaces-$version-darwin-universal.pkg"
[ -x "$app/Contents/MacOS/cua" ] || echo "warning: $app has no bundled cua CLI (build with the sidecar config)" >&2

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/root/Applications" "$work/scripts" "$work/pkgs"
ditto "$app" "$work/root/Applications/Cua Spaces.app"
cp "$res/postinstall" "$work/scripts/postinstall"
chmod 0755 "$work/scripts/postinstall"
if [ -n "$mode" ]; then
    printf '%s\n' "$mode" >"$work/scripts/spaces-install-mode"
fi

# Keep the app where the package puts it (no relocation to a moved copy).
pkgbuild --analyze --root "$work/root" "$work/components.plist" >/dev/null
plutil -replace 0.BundleIsRelocatable -bool NO "$work/components.plist"

pkgbuild --quiet --root "$work/root" --component-plist "$work/components.plist" \
    --identifier com.trycua.spaces.app --version "$version" --install-location / \
    --scripts "$work/scripts" "$work/pkgs/cua-spaces-component.pkg"
sed -e "s/__VERSION__/$version/g" -e "s/__MIN_OS__/$min_os/g" "$res/distribution.xml" >"$work/distribution.xml"
mkdir -p "$(dirname "$out")"
if [ -n "$sign" ]; then
    productbuild --quiet --distribution "$work/distribution.xml" --package-path "$work/pkgs" \
        --sign "$sign" "$out"
else
    productbuild --quiet --distribution "$work/distribution.xml" --package-path "$work/pkgs" "$out"
fi
echo "built $out"
