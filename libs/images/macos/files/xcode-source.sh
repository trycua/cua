# shellcheck shell=bash
# Sourced by build.sh and derive-local.sh: find an Xcode on this Mac and stage
# it for the guest. Needs no Apple credentials: it only uses an Xcode that is
# already here (installed, or an .xip somebody downloaded).
#   XCODE   an explicit Xcode.app or .xip (optional)
plist() { /usr/libexec/PlistBuddy -c "Print :$2" "$1/Contents/Info.plist" 2>/dev/null; }

# find_xcode prints the Xcode to use: $XCODE, $CUA_XCODE_APP, the one xcode-select
# names, else the newest /Applications/Xcode*.app, else an Xcode*.xip in
# $CUA_XCODE_CACHE (default ~/XcodesCache) or ~/Downloads. Fails when none.
find_xcode() {
    local c best="" bestv="" v sel
    if [ -n "${XCODE:-}" ]; then echo "$XCODE"; return 0; fi
    if [ -n "${CUA_XCODE_APP:-}" ]; then echo "$CUA_XCODE_APP"; return 0; fi
    sel="$(xcode-select -p 2>/dev/null || true)"
    case "$sel" in */Xcode*.app/Contents/Developer) c="${sel%/Contents/Developer}"; [ -d "$c" ] && { echo "$c"; return 0; } ;; esac
    for c in /Applications/Xcode*.app; do
        [ -d "$c" ] || continue
        v="$(plist "$c" CFBundleShortVersionString || true)"; [ -n "$v" ] || continue
        if [ -z "$best" ] || [ "$(printf '%s\n%s\n' "$bestv" "$v" | sort -V | tail -1)" = "$v" ]; then best="$c" bestv="$v"; fi
    done
    [ -z "$best" ] || { echo "$best"; return 0; }
    for c in "${CUA_XCODE_CACHE:-$HOME/XcodesCache}"/Xcode*.xip "$HOME"/Downloads/Xcode*.xip; do
        [ -f "$c" ] && { echo "$c"; return 0; }
    done
    return 1
}

# stage_xcode SRC DIR puts the Xcode into DIR as one file the guest can read
# over the share. The guest's virtiofs cannot follow an app bundle's framework
# links, but a zip keeps them, the signatures and the extended attributes; an
# .xip goes in as it is.
stage_xcode() {
    case "$1" in
        *.app) ditto -c -k --sequesterRsrc --keepParent "$1" "$2/$(basename "$1").zip" ;;
        *) cp -c "$1" "$2/" 2>/dev/null || cp "$1" "$2/" ;;
    esac
}
