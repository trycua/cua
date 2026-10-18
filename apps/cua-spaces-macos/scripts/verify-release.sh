#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Checks a Cua Spaces release the way Gatekeeper, Windows, the updaters and
# the Keyvault will see it. CD runs it on the signed build before anything is
# published; it also runs on a downloaded release. It mounts disk images
# read-only and installs nothing.
#
# SwiftUI disk image: the image and the app are Developer ID signed,
# notarized and stapled (spctl -t install, stapler validate, strict codesign;
# the image's com.apple.FinderInfo on the app bundle's root is the one
# allowed detritus); the app, its bundled `cua`, the SDK library and the
# updater (Sparkle.framework, its XPC services, Autoupdate and Updater.app)
# carry the Cua team, the hardened runtime, a secure timestamp and the
# expected identifiers and entitlements; bundle id and versions; the
# updater's feed and key; both architectures. It then launches the app
# (scripts/check-launch.sh, no window) and runs a Keyvault smoke: the bundled
# `cua daemon` with a throwaway CUA_HOME (under ~/.cache, deleted afterwards;
# never ~/.cua, no loopback port, telemetry off) answers the bundled
# `cua keyvault status`, so client and daemon verify each other as Cua. It
# reads no keychain and creates no vault.
#
# Electron disk images (Cua-Spaces-<v>-<arch>.dmg): the same image and app
# signature checks, bundle id, versions, architectures, the bundled `cua`'s
# Keyvault requirement, and the updater feed baked into app-update.yml.
#
# Windows installers (Cua-Spaces-Setup-<v>*.exe): Authenticode verifies with
# osslsigncode (chain to the pinned Microsoft Identity Verification Root
# 2020, certs/), the signer is "Cua AI, Inc." and it has a timestamp.
#
# Sparkle appcast (cua-spaces-appcast.xml): every X.Y.Z-suffix item is on the
# beta channel and every X.Y.Z item on none (so Stable apps never see a
# prerelease), and this release's item passes scripts/verify-appcast.sh.
#
# Electron feeds (beta*.yml / latest*.yml): each names this version, its
# files are this version's (and match the local files' sha512 and size), and
# a prerelease's feeds are beta*.yml.
#
# Release manifest (--dir holding every asset of the published release):
# checksums.txt and release-artifacts.json list every artifact, Windows and
# Linux included, with its sha256 (scripts/install/check_release_manifest.py).
#
#   scripts/verify-release.sh cua-spaces-0.8.0-darwin-universal.dmg
#   scripts/verify-release.sh --dir DIR --version 0.8.0-beta.6 [--expect swift,electron,windows,appcast,feed,manifest]
#       [--repo trycua/cua] [--team YCK386LBJ7] [--arch universal|arm64|x86_64]
#       [--windows-signer "Cua AI, Inc."] [--no-launch] [--no-keyvault-smoke]
#
# --dir finds every asset of --version in DIR by its release name; --expect
# lists the kinds that must be there (default: swift). Single files:
# --swift-dmg, --electron-dmg, --windows-exe, --appcast, --feed (repeatable).
# --repo checks that both updaters' feeds are that repository's.
#
# Exits non-zero when any check fails; prints every check either way.
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
team="YCK386LBJ7"
arch=universal
smoke=1
launch=1
version=""
repo=""
dir=""
expect="swift"
signer="Cua AI, Inc."
swift_dmg=""
electron_dmgs=()
windows_exes=()
appcast=""
feeds=()
while [ $# -gt 0 ]; do
  case "$1" in
    --team) team="$2"; shift ;;
    --arch) arch="$2"; shift ;;
    --version) version="$2"; shift ;;
    --repo) repo="$2"; shift ;;
    --dir) dir="$2"; shift ;;
    --expect) expect="$2"; shift ;;
    --windows-signer) signer="$2"; shift ;;
    --swift-dmg) swift_dmg="$2"; shift ;;
    --electron-dmg) electron_dmgs+=("$2"); shift ;;
    --windows-exe) windows_exes+=("$2"); shift ;;
    --appcast) appcast="$2"; shift ;;
    --feed) feeds+=("$2"); shift ;;
    --no-keyvault-smoke) smoke=0 ;;
    --no-launch) launch=0 ;;
    -h | --help) sed -n '5,52p' "$0"; exit 0 ;;
    -*) echo "unknown option: $1" >&2; exit 2 ;;
    *) swift_dmg="$1" ;;
  esac
  shift
done
case "$arch" in universal) archs="arm64 x86_64" ;; arm64 | x86_64) archs="$arch" ;; *) echo "bad --arch" >&2; exit 2 ;; esac
if [ -n "$dir" ]; then
  [ -d "$dir" ] || { echo "no directory $dir" >&2; exit 2; }
  [ -n "$version" ] || { echo "--dir needs --version" >&2; exit 2; }
  [ -f "$dir/cua-spaces-$version-darwin-universal.dmg" ] && swift_dmg="$dir/cua-spaces-$version-darwin-universal.dmg"
  for f in "$dir/Cua-Spaces-$version"-*.dmg; do [ -f "$f" ] && electron_dmgs+=("$f"); done
  for f in "$dir/Cua-Spaces-Setup-$version".exe "$dir/Cua-Spaces-Setup-$version"-*.exe; do
    [ -f "$f" ] && windows_exes+=("$f")
  done
  [ -f "$dir/cua-spaces-appcast.xml" ] && appcast="$dir/cua-spaces-appcast.xml"
  for f in "$dir"/beta*.yml "$dir"/latest*.yml; do [ -f "$f" ] && feeds+=("$f"); done
fi
if [ -z "$swift_dmg$appcast" ] && [ ${#electron_dmgs[@]} -eq 0 ] && [ ${#windows_exes[@]} -eq 0 ] &&
  [ ${#feeds[@]} -eq 0 ]; then
  echo "usage: $0 <path/to.dmg> | --dir DIR --version V (see --help)" >&2
  exit 2
fi
[ -z "$swift_dmg" ] || [ -f "$swift_dmg" ] || { echo "no file $swift_dmg" >&2; exit 2; }

failed=0
section() { printf '\n== %s\n' "$1"; }
check() { # check NAME CMD...: run CMD, show its output, record the result
  local name="$1"; shift
  local output
  output="$("$@" 2>&1)"
  local rc=$?
  [ -n "$output" ] && printf '%s\n' "$output" | sed 's/^/   /'
  if [ $rc -eq 0 ]; then echo "PASS $name"; else echo "FAIL $name"; failed=1; fi
}
fail() { echo "FAIL $1"; failed=1; }
plist_key() { /usr/libexec/PlistBuddy -c "Print :$1" "$2" 2>/dev/null; }

mounts=()
tmp="$(mktemp -d)"
kv_home=""
daemon_pid=""
unmount_all() {
  local m
  for m in ${mounts[@]+"${mounts[@]}"}; do
    hdiutil detach -quiet "$m" >/dev/null 2>&1 || hdiutil detach -force -quiet "$m" >/dev/null 2>&1
    rmdir "$m" 2>/dev/null
  done
  mounts=()
}
cleanup() {
  [ -n "$daemon_pid" ] && kill "$daemon_pid" 2>/dev/null && wait "$daemon_pid" 2>/dev/null
  [ -n "$kv_home" ] && rm -rf "$kv_home"
  unmount_all
  # Never recurse into an image that is still mounted.
  mount | grep -qF "$tmp/" || rm -rf "$tmp"
}
trap cleanup EXIT

# mount_dmg DMG: mounts it read-only at $mnt. One image at a time: each
# section unmounts its own (verify-appcast.sh mounts the image again).
mount_dmg() {
  mnt="$(mktemp -d "$tmp/mnt.XXXXXX")"
  hdiutil attach -quiet -readonly -nobrowse -noautoopen -mountpoint "$mnt" "$1" >/dev/null || return 1
  mounts+=("$mnt")
}

# Strict, deep codesign. A disk image's Finder layout leaves
# com.apple.FinderInfo on the app bundle's root, which strict verification
# calls detritus although Gatekeeper accepts it; check a copy without that one
# attribute (everything else, nested attributes included, stays strict).
strict_app() {
  codesign --verify --deep --strict --verbose=2 "$1" && return 0
  xattr -p com.apple.FinderInfo "$1" >/dev/null 2>&1 || return 1
  echo "(retrying without the bundle root's com.apple.FinderInfo, the disk image's layout)"
  local copy
  copy="$(mktemp -d "$tmp/copy.XXXXXX")/${1##*/}"
  ditto "$1" "$copy" && xattr -d com.apple.FinderInfo "$copy" &&
    codesign --verify --deep --strict --verbose=2 "$copy"
}

image_checks() { # image_checks DMG
  check "image signature (codesign --verify --strict)" codesign --verify --strict --verbose=2 "$1"
  check "image Gatekeeper, install assessment (spctl -t install)" spctl -a -vv -t install "$1"
  check "image Gatekeeper, primary signature (spctl -t open)" \
    spctl -a -vv -t open --context context:primary-signature "$1"
  check "image notarization ticket stapled" xcrun stapler validate "$1"
  check "image team is $team" sh -c "codesign -dvv \"\$1\" 2>&1 | grep -qx 'TeamIdentifier=$team'" _ "$1"
}

signature_checks() { # signature_checks PATH LABEL
  section "signature: $2"
  local info
  info="$(codesign -dvv "$1" 2>&1)"
  printf '%s\n' "$info" | grep -E '^(Identifier|Format|CodeDirectory|Authority|Timestamp|TeamIdentifier|Runtime Version)' | sed 's/^/   /'
  check "Developer ID Application authority" sh -c "printf '%s\n' \"\$1\" | grep -q '^Authority=Developer ID Application: .*($team)$'" _ "$info"
  check "team $team" sh -c "printf '%s\n' \"\$1\" | grep -qx 'TeamIdentifier=$team'" _ "$info"
  check "hardened runtime" sh -c "printf '%s\n' \"\$1\" | grep -q '^CodeDirectory.*flags=.*runtime'" _ "$info"
  check "secure timestamp" sh -c "printf '%s\n' \"\$1\" | grep -q '^Timestamp='" _ "$info"
}

keyvault_requirement() { # keyvault_requirement PATH IDENTIFIER
  codesign --verify -R="anchor apple generic and certificate leaf[subject.OU] = \"$team\" and identifier \"$2\"" "$1"
}

expect_eq() { # expect_eq NAME GOT WANT
  if [ "$2" = "$3" ]; then echo "PASS $1 ($2)"; else fail "$1: got '$2', want '$3'"; fi
}

# Versions every app carries: CuaVersion is the full version, CFBundleVersion
# X.Y.Z.<build>, CFBundleShortVersionString X.Y.Z (SwiftUI) or the full
# version (Electron).
version_checks() { # version_checks PLIST SHORT_WANTED
  local short bundle cua
  short="$(plist_key CFBundleShortVersionString "$1")"
  bundle="$(plist_key CFBundleVersion "$1")"
  cua="$(plist_key CuaVersion "$1")"
  printf '   version %s (%s), CuaVersion %s\n' "$short" "$bundle" "${cua:-(none)}"
  expect_eq "bundle id" "$(plist_key CFBundleIdentifier "$1")" "com.trycua.spaces.macos"
  if [ -n "$version" ]; then
    expect_eq "CuaVersion is the release version" "$cua" "$version"
    expect_eq "CFBundleShortVersionString" "$short" "$2"
    expect_eq "CFBundleVersion is X.Y.Z.<build>" \
      "$(sed -E 's/^([0-9]+\.[0-9]+\.[0-9]+)\.[0-9]+$/\1/' <<<"$bundle")" "${version%%-*}"
  fi
}

# --- SwiftUI app --------------------------------------------------------------
swift_app=""
if [ -n "$swift_dmg" ]; then
  section "SwiftUI disk image: $swift_dmg"
  image_checks "$swift_dmg"
  if ! mount_dmg "$swift_dmg"; then
    fail "could not mount $swift_dmg"
  else
    swift_app="$(find "$mnt" -maxdepth 1 -name '*.app' | head -n 1)"
  fi
  if [ -z "$swift_app" ]; then
    fail "no .app in $swift_dmg"
  else
    app="$swift_app"
    [ -z "$version" ] && version="$(plist_key CuaVersion "$app/Contents/Info.plist")"
    section "SwiftUI app: ${app##*/}"
    version_checks "$app/Contents/Info.plist" "${version%%-*}"
    check "app signature, deep and strict" strict_app "$app"
    check "app Gatekeeper (spctl)" spctl -a -vv "$app"
    check "app notarization ticket stapled" xcrun stapler validate "$app"
    # The Keyvault's production caller requirement (cua-keyvault caller.rs).
    check "app meets the Keyvault requirement (team $team, com.trycua.spaces.macos)" \
      keyvault_requirement "$app" com.trycua.spaces.macos
    check "bundled cua meets the Keyvault requirement (team $team, com.trycua.cua)" \
      keyvault_requirement "$app/Contents/MacOS/cua" com.trycua.cua

    section "updater (Sparkle)"
    plist="$app/Contents/Info.plist"
    feed="$(plist_key SUFeedURL "$plist")"
    printf '   SUFeedURL %s\n' "${feed:-(none)}"
    check "SUFeedURL is https" sh -c "case '$feed' in https://*) exit 0 ;; *) exit 1 ;; esac"
    if [ -n "$repo" ]; then
      expect_eq "SUFeedURL is $repo's" "$feed" \
        "https://github.com/$repo/releases/download/cua-spaces-latest/cua-spaces-appcast.xml"
    fi
    check "SUPublicEDKey is a 32-byte Ed25519 key" \
      sh -c "/usr/libexec/PlistBuddy -c 'Print :SUPublicEDKey' '$plist' | base64 -d 2>/dev/null | wc -c | grep -qx ' *32'"
    sparkle="$app/Contents/Frameworks/Sparkle.framework"
    sparkle_parts=(
      "$sparkle/Versions/B/XPCServices/Installer.xpc"
      "$sparkle/Versions/B/XPCServices/Downloader.xpc"
      "$sparkle/Versions/B/Autoupdate"
      "$sparkle/Versions/B/Updater.app"
      "$sparkle"
    )
    for f in "${sparkle_parts[@]}"; do check "present: ${f#"$app"/}" test -e "$f"; done

    for f in "$app" "$app/Contents/MacOS/cua" "$app/Contents/Frameworks/libcua_sdk.dylib" "${sparkle_parts[@]}"; do
      signature_checks "$f" "${f#"$mnt"/}"
    done

    for f in "$app" "$app/Contents/MacOS/cua"; do
      section "entitlements: ${f#"$mnt"/}"
      ents="$(codesign -d --entitlements - --xml "$f" 2>/dev/null | plutil -convert xml1 -o - - 2>/dev/null)"
      printf '%s\n' "${ents:-   (none)}" | sed 's/^/   /'
      check "no get-task-allow (debuggable builds are not notarizable)" \
        sh -c "! printf '%s' \"\$1\" | grep -q 'get-task-allow'" _ "$ents"
    done

    if [ "$launch" = 1 ]; then
      section "launch"
      # Libraries resolve inside the bundle and pass library validation; the
      # app starts and exits before any window (throwaway HOME, telemetry off).
      check "the app launches (scripts/check-launch.sh)" "$here/check-launch.sh" "$app"
    fi

    section "architectures"
    for f in "$app/Contents/MacOS/CuaSpacesMac" "$app/Contents/MacOS/cua" "$app/Contents/Frameworks/libcua_sdk.dylib" \
      "$sparkle/Versions/B/Sparkle" "$sparkle/Versions/B/Autoupdate"; do
      for a in $archs; do check "${f##*/} has $a ($(lipo -archs "$f"))" lipo "$f" -verify_arch "$a"; done
    done

    if [ "$smoke" = 1 ]; then
      section "Keyvault smoke (bundled cua daemon, throwaway CUA_HOME)"
      cua="$app/Contents/MacOS/cua"
      mkdir -p "$HOME/.cache"
      # Inside the real home: the Keyvault distrusts a CUA_HOME outside it.
      kv_home="$(mktemp -d "$HOME/.cache/cua-verify-release.XXXXXX")"
      chmod 700 "$kv_home"
      CUA_HOME="$kv_home" CUA_TELEMETRY=0 DO_NOT_TRACK=1 \
        "$cua" daemon start --foreground --socket "$kv_home/cua.sock" --loopback off \
        >"$kv_home/daemon.out" 2>&1 &
      daemon_pid=$!
      for _ in $(seq 1 80); do [ -S "$kv_home/keyvault.sock" ] && break; sleep 0.25; done
      check "the bundled daemon serves the Keyvault socket" test -S "$kv_home/keyvault.sock"
      check "cua keyvault status: client and daemon verify each other as Cua" \
        env CUA_HOME="$kv_home" CUA_TELEMETRY=0 DO_NOT_TRACK=1 \
        "$cua" keyvault status --daemon "$kv_home/cua.sock"
      kill "$daemon_pid" 2>/dev/null && wait "$daemon_pid" 2>/dev/null
      daemon_pid=""
    fi
  fi
  unmount_all
fi

# --- Electron app -------------------------------------------------------------
for dmg in ${electron_dmgs[@]+"${electron_dmgs[@]}"}; do
  section "Electron disk image: $dmg"
  image_checks "$dmg"
  case "$dmg" in *-arm64.dmg) e_archs="arm64" ;; *-x64.dmg | *-x86_64.dmg) e_archs="x86_64" ;; *) e_archs="arm64 x86_64" ;; esac
  if ! mount_dmg "$dmg"; then fail "could not mount $dmg"; continue; fi
  app="$mnt/Cua Spaces.app"
  [ -d "$app" ] || { fail "no Cua Spaces.app in $dmg"; unmount_all; continue; }
  section "Electron app: ${dmg##*/}"
  version_checks "$app/Contents/Info.plist" "$version"
  check "app signature, deep and strict" strict_app "$app"
  check "app Gatekeeper (spctl)" spctl -a -vv "$app"
  check "app notarization ticket stapled" xcrun stapler validate "$app"
  check "app meets the Keyvault requirement (team $team, com.trycua.spaces.macos)" \
    keyvault_requirement "$app" com.trycua.spaces.macos
  e_cua="$app/Contents/Resources/native/cua"
  check "bundled cua meets the Keyvault requirement (team $team, com.trycua.cua)" \
    keyvault_requirement "$e_cua" com.trycua.cua
  signature_checks "$app" "${dmg##*/}: Cua Spaces.app"
  signature_checks "$e_cua" "${dmg##*/}: Contents/Resources/native/cua"
  section "Electron updater (app-update.yml)"
  update_yml="$app/Contents/Resources/app-update.yml"
  sed 's/^/   /' "$update_yml" 2>/dev/null
  e_url="$(sed -n 's/^url: *//p' "$update_yml" 2>/dev/null | tr -d "'\"")"
  check "feed URL is https" sh -c "case '$e_url' in https://*) exit 0 ;; *) exit 1 ;; esac"
  [ -n "$repo" ] && expect_eq "feed URL is $repo's" "$e_url" "https://github.com/$repo/releases/download/cua-spaces-latest"
  section "Electron architectures: ${dmg##*/}"
  for f in "$app/Contents/MacOS/Cua Spaces" "$e_cua"; do
    for a in $e_archs; do check "${f##*/} has $a ($(lipo -archs "$f"))" lipo "$f" -verify_arch "$a"; done
  done
  unmount_all
done

# --- Windows installers -------------------------------------------------------
if [ ${#windows_exes[@]} -gt 0 ]; then
  root_ca="$here/certs/microsoft-identity-verification-root-2020.pem"
  if ! command -v osslsigncode >/dev/null; then
    section "Windows installers"
    fail "osslsigncode is not installed (brew install osslsigncode)"
  fi
  for exe in "${windows_exes[@]}"; do
    command -v osslsigncode >/dev/null || break
    section "Windows installer: $exe"
    out="$(osslsigncode verify -CAfile "$root_ca" -TSA-CAfile "$root_ca" -in "$exe" 2>&1)"
    rc=$?
    printf '%s\n' "$out" | grep -E "Subject:|Issuer :|Timestamp time:|verification:|^(Succeeded|Failed)" | sort -u | sed 's/^/   /'
    [ $rc -eq 0 ] && echo "PASS Authenticode signature and chain verify (osslsigncode)" ||
      fail "Authenticode signature and chain verify (osslsigncode, exit $rc)"
    # The leaf: the first Subject after "Signer's certificate".
    leaf="$(printf '%s\n' "$out" | awk '/^Signer.s certificate:/ {s=1} s && /Subject:/ {sub(/^[ \t]*Subject: */, ""); print; exit}')"
    want_cn="CN=$(printf '%s' "$signer" | sed 's/,/\\,/g'),"
    case "$leaf" in
      "$want_cn"*) echo "PASS signer is $signer" ;;
      *) fail "signer is $signer: got '$leaf'" ;;
    esac
    printf '%s\n' "$out" | grep -q "Timestamp time:" && echo "PASS timestamped" || fail "no timestamp"
    case "${exe##*/}" in *"$version"*) echo "PASS file name carries $version" ;; *) fail "file name does not carry $version" ;; esac
  done
fi

# --- Sparkle appcast ----------------------------------------------------------
if [ -n "$appcast" ]; then
  section "Sparkle appcast: $appcast"
  item_url="$(python3 - "$appcast" "$version" <<'PY'
import sys
import xml.etree.ElementTree as ET
path, version = sys.argv[1:]
SP = "{http://www.andymatuschak.org/xml-namespaces/sparkle}"
bad = 0
found = []
for item in ET.parse(path).iter("item"):
    short = (item.findtext(SP + "shortVersionString") or "").strip()
    channel = (item.findtext(SP + "channel") or "").strip()
    want = "beta" if "-" in short else ""
    if channel != want:
        print(f"FAIL item {short}: channel '{channel}', want '{want}' (prereleases are Beta only)", file=sys.stderr)
        bad = 1
    if short == version:
        enclosure = item.find("enclosure")
        found.append(enclosure.get("url") if enclosure is not None else "")
if version and len(found) != 1:
    print(f"FAIL expected one item for {version}, found {len(found)}", file=sys.stderr)
    bad = 1
elif found:
    print(found[0])
sys.exit(bad)
PY
)"
  rc=$?
  [ $rc -eq 0 ] && echo "PASS only X.Y.Z-suffix items are on the beta channel, and one item is $version" ||
    { fail "appcast channels or this release's item"; }
  if [ -n "$item_url" ]; then
    echo "   item: $item_url"
    item_dmg=""
    electron_flag=()
    case "${item_url##*/}" in
      "cua-spaces-$version-darwin-universal.dmg") item_dmg="$swift_dmg" ;;
      "Cua-Spaces-$version-universal.dmg")
        electron_flag=(--electron)
        for f in ${electron_dmgs[@]+"${electron_dmgs[@]}"}; do [ "${f##*/}" = "${item_url##*/}" ] && item_dmg="$f"; done ;;
    esac
    if [ -n "$repo" ]; then
      expect_eq "item downloads from $repo" "${item_url%/*}/" "https://github.com/$repo/releases/download/cua-spaces-v$version/"
    fi
    if [ -n "$item_dmg" ] && [ -f "$item_dmg" ]; then
      check "this release's item (scripts/verify-appcast.sh)" "$here/verify-appcast.sh" \
        --appcast "$appcast" --dmg "$item_dmg" --url "$item_url" --version "$version" ${electron_flag[@]+"${electron_flag[@]}"}
    else
      fail "the item's disk image ${item_url##*/} is not here to verify"
    fi
  fi
fi

# --- Electron update feeds ----------------------------------------------------
if [ ${#feeds[@]} -gt 0 ]; then
  section "Electron update feeds"
  # Local copies of the files the feeds name, to compare sha512 and size.
  locals=()
  for f in ${electron_dmgs[@]+"${electron_dmgs[@]}"} ${windows_exes[@]+"${windows_exes[@]}"}; do locals+=("$f"); done
  [ -n "$dir" ] && for f in "$dir"/*.zip "$dir"/*.AppImage "$dir"/*.deb; do [ -f "$f" ] && locals+=("$f"); done
  check "feeds name $version and its files" python3 - "$version" "$repo" ${locals[@]+"${locals[@]}"} -- "${feeds[@]}" <<'PY'
import base64, hashlib, os, re, sys
args = sys.argv[1:]
version, repo = args[0], args[1]
split = args.index("--")
local = {os.path.basename(p): p for p in args[2:split]}
feeds = args[split + 1:]
prerelease = "-" in version
bad = 0
def fail(msg):
    global bad
    print(f"FAIL {msg}")
    bad = 1
for feed in feeds:
    name = os.path.basename(feed)
    text = open(feed).read()
    top = dict(re.findall(r"^(version|path|sha512|releaseDate): *'?([^'\n]*)'?$", text, re.M))
    files = []
    # The files: list, up to the next top-level key.
    section = re.search(r"^files:\n((?:[ -].*\n?)*)", text, re.M)
    for block in re.split(r"^  - ", section.group(1) if section else "", flags=re.M)[1:]:
        entry = dict(re.findall(r"(url|sha512|size): *([^\n]+)", block))
        files.append(entry)
    if name.startswith("beta") != prerelease and name.startswith(("beta", "latest")):
        # A prerelease writes beta*.yml; a stable release (the cutover) latest*.yml.
        fail(f"{name}: a {'prerelease' if prerelease else 'stable release'} must not write it")
    if top.get("version") != version:
        fail(f"{name}: version {top.get('version')!r}, want {version!r}")
    if not files:
        fail(f"{name}: no files")
    for entry in files:
        url = entry.get("url", "")
        base = url.rsplit("/", 1)[-1]
        if version not in base:
            fail(f"{name}: {base} is not a {version} file")
        if "://" in url:
            want = f"/cua-spaces-v{version}/"
            if want not in url:
                fail(f"{name}: {url} is not on the cua-spaces-v{version} release")
            if repo and not url.startswith(f"https://github.com/{repo}/releases/download/"):
                fail(f"{name}: {url} is not on {repo}")
        if base in local:
            data = open(local[base], "rb").read()
            digest = base64.b64encode(hashlib.sha512(data).digest()).decode()
            if digest != entry.get("sha512", "").strip():
                fail(f"{name}: {base} sha512 does not match the file")
            elif str(len(data)) != entry.get("size", "").strip():
                fail(f"{name}: {base} size does not match the file")
            else:
                print(f"   {name}: {base} sha512 and size match")
    path = top.get("path", "")
    if path and path.rsplit("/", 1)[-1] not in {e.get("url", "").rsplit("/", 1)[-1] for e in files}:
        fail(f"{name}: path {path} is not one of its files")
    print(f"   {name}: version {top.get('version')}, {len(files)} files")
sys.exit(bad)
PY
  channel=latest
  [[ "$version" == *-* ]] && channel=beta
  have=" "
  for f in "${feeds[@]}"; do have="$have${f##*/} "; done
  # The feed of each platform whose installers are here.
  wants=()
  [ ${#electron_dmgs[@]} -gt 0 ] && wants+=("$channel-mac.yml")
  [ ${#windows_exes[@]} -gt 0 ] && wants+=("$channel.yml")
  for want in ${wants[@]+"${wants[@]}"}; do
    case "$have" in *" $want "*) echo "PASS feed present: $want" ;; *) fail "feed missing: $want" ;; esac
  done
fi

# --- checksums.txt and release-artifacts.json -------------------------------
manifest=""
if [ -n "$dir" ] && { [ -f "$dir/release-artifacts.json" ] || [ -f "$dir/checksums.txt" ]; }; then
  manifest=1
  section "release manifest: checksums.txt, release-artifacts.json"
  base_args=()
  [ -n "$repo" ] && base_args=(--base-url "https://github.com/$repo/releases/download/cua-spaces-v$version")
  check "every published artifact is listed with its sha256" \
    python3 "$here/../../../scripts/install/check_release_manifest.py" --dir "$dir" --component app \
    --version "$version" ${base_args[@]+"${base_args[@]}"}
fi

# --- What had to be there -----------------------------------------------------
section "expected assets ($expect)"
for kind in ${expect//,/ }; do
  case "$kind" in
    swift) [ -n "$swift_dmg" ] && echo "PASS SwiftUI disk image" || fail "no SwiftUI disk image" ;;
    electron) [ ${#electron_dmgs[@]} -gt 0 ] && echo "PASS Electron disk images (${#electron_dmgs[@]})" || fail "no Electron disk images" ;;
    windows) [ ${#windows_exes[@]} -gt 0 ] && echo "PASS Windows installers (${#windows_exes[@]})" || fail "no Windows installers" ;;
    appcast) [ -n "$appcast" ] && echo "PASS Sparkle appcast" || fail "no Sparkle appcast" ;;
    feed) [ ${#feeds[@]} -gt 0 ] && echo "PASS Electron feeds (${#feeds[@]})" || fail "no Electron feeds" ;;
    manifest) [ -n "$manifest" ] && echo "PASS release manifest" || fail "no checksums.txt or release-artifacts.json" ;;
    *) fail "unknown --expect kind: $kind" ;;
  esac
done

echo
if [ $failed -eq 0 ]; then echo "ALL CHECKS PASSED"; else echo "SOME CHECKS FAILED"; fi
exit $failed
