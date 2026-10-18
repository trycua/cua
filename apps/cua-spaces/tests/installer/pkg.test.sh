#!/bin/sh
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Tests the macOS .pkg without installing it: builds a package from a fake
# app, expands it and checks the payload and scripts, then runs postinstall
# against a temp "volume" ($3). macOS only (pkgbuild/productbuild/pkgutil).
set -u
here="$(cd "$(dirname "$0")/../.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
pass=0
fail=0
check() {
    name="$1"
    shift
    if "$@"; then
        pass=$((pass + 1))
        echo "ok   $name"
    else
        fail=$((fail + 1))
        echo "FAIL $name"
    fi
}
if [ "$(uname -s)" != Darwin ]; then
    echo "skip: macOS only"
    exit 0
fi

app="$work/Cua Spaces.app"
mkdir -p "$app/Contents/MacOS"
printf '#!/bin/sh\necho cua-spaces\n' >"$app/Contents/MacOS/cua-spaces"
printf '#!/bin/sh\necho "cua 1.2.3"\n' >"$app/Contents/MacOS/cua"
chmod +x "$app/Contents/MacOS/"*
cat >"$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.trycua.spaces.prototype</string>
<key>CFBundleExecutable</key><string>cua-spaces</string>
<key>CFBundleShortVersionString</key><string>1.2.3</string>
<key>CFBundleVersion</key><string>1.2.3</string>
</dict></plist>
PLIST

pkg="$work/out/cua-spaces-1.2.3-darwin-universal.pkg"
"$here/scripts/build-pkg.sh" --app "$app" --version 1.2.3 --mode host --out "$pkg" >"$work/build.log" 2>&1
check "build-pkg.sh builds a product archive" [ -f "$pkg" ]
pkgutil --expand "$pkg" "$work/x" >/dev/null 2>&1
check "distribution names the component and version" grep -q 'version="1.2.3"' "$work/x/Distribution"
comp="$work/x/cua-spaces-component.pkg"
check "payload holds the app and its bundled cua" sh -c "pkgutil --payload-files '$pkg' 2>/dev/null | grep -q './Applications/Cua Spaces.app/Contents/MacOS/cua\$'"
cp "$comp/Scripts/postinstall" "$comp/Scripts/spaces-install-mode" "$work/" 2>/dev/null
check "scripts carry postinstall and the baked mode" sh -c "[ -x '$work/postinstall' ] && grep -qx host '$work/spaces-install-mode'"
check "app is not relocatable" sh -c "! grep -q '<relocate>' '$comp/PackageInfo'"

# postinstall against a temp volume: links the CLI and writes the mode.
vol="$work/vol"
mkdir -p "$vol/Applications"
cp -R "$app" "$vol/Applications/"
sh "$work/postinstall" "$pkg" "/" "$vol" >"$work/post.log" 2>&1
check "postinstall links /usr/local/bin/cua into the app" \
    [ "$(readlink "$vol/usr/local/bin/cua")" = "/Applications/Cua Spaces.app/Contents/MacOS/cua" ]
check "postinstall writes the system install-mode file" \
    grep -qx host "$vol/Library/Application Support/Cua/spaces-install-mode"
rm "$vol/usr/local/bin/cua"
printf '#!/bin/sh\n' >"$vol/usr/local/bin/cua"
sh "$work/postinstall" "$pkg" "/" "$vol" >"$work/post.log" 2>&1
check "postinstall leaves an unrelated cua alone" sh -c "[ ! -L '$vol/usr/local/bin/cua' ] && grep -q 'leaving the existing' '$work/post.log'"

echo
echo "$pass passed, $fail failed"
[ "$fail" = 0 ]
