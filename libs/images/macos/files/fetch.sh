# shellcheck shell=bash
# Sourced by build.sh and derive-local.sh: the checksummed download cache.
#   CACHE            the host cache directory
#   SETUP            the share the guest reads (cache/ inside it)
#   log              a logging function
# fetch URL SHA256 downloads once into $CACHE, checks the hash and links the
# file into $SETUP/cache.
fetch() {
    local f; f="$CACHE/$(basename "$1")"
    if [ ! -f "$f" ] || [ "$(shasum -a 256 "$f" | cut -d' ' -f1)" != "$2" ]; then
        log "fetching $1"
        curl -fsSL --retry 3 -o "$f.part" "$1"
        [ "$(shasum -a 256 "$f.part" | cut -d' ' -f1)" = "$2" ] ||
            { rm -f "$f.part"; echo "checksum mismatch: $1" >&2; exit 1; }
        mv "$f.part" "$f"
    fi
    ln -f "$f" "$SETUP/cache/" 2>/dev/null || cp "$f" "$SETUP/cache/"
}
# The downloads of the xcode tier's tools (everything but Xcode itself).
fetch_xcode_tools() {
    fetch "$SCCACHE_URL" "$SCCACHE_SHA256"
    fetch "$XCODEGEN_URL" "$XCODEGEN_SHA256"
}
