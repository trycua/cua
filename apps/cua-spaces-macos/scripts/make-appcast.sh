#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Adds one release to the Sparkle appcast (the macOS app's update feed):
# Sparkle's generate_appcast signs the disk image with the EdDSA key and
# writes its item into the previous appcast (older items kept, at most 10
# per channel), then the item shows the full version (0.2.0-staging.6; the
# bundle's CFBundleShortVersionString is only X.Y.Z) and
# scripts/verify-appcast.sh checks the result the way the app will.
#
#   CUA_SPACES_SPARKLE_ED_PRIVATE_KEY=... scripts/make-appcast.sh \
#       --dmg cua-spaces-0.2.0-darwin-universal.dmg --version 0.2.0 \
#       --download-base https://github.com/trycua/cua/releases/download/cua-spaces-v0.2.0/ \
#       [--previous cua-spaces-appcast.xml] [--notes notes.md] --out cua-spaces-appcast.xml
#       [--sparkle-bin DIR]
#
# A version with a suffix (X.Y.Z-suffix) goes to the `beta` channel, which
# only apps set to "Update to: Beta" see; X.Y.Z goes to everyone. --notes
# (Markdown) becomes the item's release notes. The key (the base64 Ed25519
# seed from Sparkle's key format) is read from the environment and handed
# to Sparkle on standard input; it is never written to disk or printed.
# --sparkle-bin defaults to the tools SwiftPM resolved for this package
# (.build/artifacts/sparkle/Sparkle/bin, Sparkle as pinned in
# Package.resolved).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dmg=""
version=""
base=""
previous=""
notes=""
out=""
bin=""
while [ $# -gt 0 ]; do
  case "$1" in
    --dmg) dmg="$2"; shift ;;
    --version) version="$2"; shift ;;
    --download-base) base="$2"; shift ;;
    --previous) previous="$2"; shift ;;
    --notes) notes="$2"; shift ;;
    --out) out="$2"; shift ;;
    --sparkle-bin) bin="$2"; shift ;;
    -h | --help) sed -n '5,27p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done
[ -f "$dmg" ] || { echo "--dmg must be a disk image" >&2; exit 2; }
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || { echo "--version must look like 1.2.3 or 1.2.3-suffix" >&2; exit 2; }
case "$base" in http://* | https://*) ;; *) echo "--download-base must be a URL" >&2; exit 2 ;; esac
case "$base" in */) ;; *) base="$base/" ;; esac
[ -n "$out" ] || { echo "--out is required" >&2; exit 2; }
[ -n "${CUA_SPACES_SPARKLE_ED_PRIVATE_KEY:-}" ] || { echo "CUA_SPACES_SPARKLE_ED_PRIVATE_KEY is not set" >&2; exit 2; }
if [ -z "$bin" ]; then
  bin="$here/.build/artifacts/sparkle/Sparkle/bin"
  [ -x "$bin/generate_appcast" ] || swift package --package-path "$here" resolve >/dev/null
fi
[ -x "$bin/generate_appcast" ] || { echo "no generate_appcast in $bin" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
name="$(basename "$dmg")"
cp "$dmg" "$work/$name"
[ -n "$previous" ] && [ -s "$previous" ] && cp "$previous" "$work/cua-spaces-appcast.xml"
[ -n "$notes" ] && [ -s "$notes" ] && cp "$notes" "$work/${name%.dmg}.md"
channel=()
[[ "$version" == *-* ]] && channel=(--channel beta)

printf '%s' "$CUA_SPACES_SPARKLE_ED_PRIVATE_KEY" |
  "$bin/generate_appcast" --ed-key-file - ${channel[@]+"${channel[@]}"} --embed-release-notes \
    --maximum-versions 10 --download-url-prefix "$base" -o "$work/cua-spaces-appcast.xml" "$work"

# The new item shows the full version.
python3 - "$work/cua-spaces-appcast.xml" "$base$name" "$version" <<'PY'
import sys
import xml.etree.ElementTree as ET
path, url, version = sys.argv[1:]
SP = "http://www.andymatuschak.org/xml-namespaces/sparkle"
ET.register_namespace("sparkle", SP)
tree = ET.parse(path)
items = [i for i in tree.iter("item") if (i.find("enclosure") is not None and i.find("enclosure").get("url") == url)]
if len(items) != 1:
    sys.exit(f"expected one appcast item for {url}, found {len(items)}")
item = items[0]
item.find("title").text = version
short = item.find(f"{{{SP}}}shortVersionString")
if short is None:
    short = ET.SubElement(item, f"{{{SP}}}shortVersionString")
short.text = version
ET.indent(tree, space="    ")
tree.write(path, encoding="utf-8", xml_declaration=True)
PY

"$here/scripts/verify-appcast.sh" --appcast "$work/cua-spaces-appcast.xml" --dmg "$work/$name" \
  --url "$base$name" --version "$version" --sparkle-bin "$bin"
mkdir -p "$(dirname "$out")"
cp "$work/cua-spaces-appcast.xml" "$out"
echo "$out"
