#!/bin/sh
# Cua installer: the `cua` CLI, the Cua Spaces app and agent extras.
#
#   curl -fsSL https://cua.ai/install.sh | sh
#   curl -fsSL https://cua.ai/install.sh | sh -s -- --select cua-driver
#   curl -fsSL https://cua.ai/install.sh | sh -s -- --cli-only --yes
#   curl -fsSL https://cua.ai/install.sh | sh -s -- --pair K7QM-4XPB-2RHD
#
# In a terminal it shows a checklist of what to install. Items:
#   cli         the cua CLI (always installed, except with --app-only)
#   spaces      the Cua Spaces app (macOS and Linux; default on both)
#   cua-driver  cua-driver MCP and skill for your agents (default off)
#   host        host this machine: `cua host setup` (default off)
#
# Options:
#   --select LIST       preselect items (comma list, e.g. cua-driver,host)
#   --only LIST         install exactly LIST plus the CLI, no checklist
#   --cli-only          install only the `cua` CLI (same as --only cli)
#   --app-only          install only the Cua Spaces app (no CLI)
#   --mode host|client  preselect the app's first-run choice (implies spaces)
#   --pair CODE         add this machine to your account with a one-time code
#                       from the Cua Spaces app (Add a machine): installs and
#                       runs `cua host setup --pair CODE`, no sign-in, no checklist
#   --relay URL         relay for --pair (default https://relay.cua.ai)
#                       A computer that already has a cua with `host setup --pair`
#                       (and the app) uses them instead of downloading again;
#                       --version, --prefix or CUA_INSTALL_REINSTALL=1 install anyway
#   --version VERSION   install this CLI release (default: latest)
#   --prefix DIR        install under DIR (DIR/bin, DIR/Applications)
#   --modify-path       add the bin dir to your shell profile without asking
#   --no-modify-path    never edit your shell profile (print the one-line command)
#   --no-onboarding     do not run `cua auth login` afterwards
#   --require-signature fail unless a signature (minisign/cosign) verifies
#   --dry-run           print what would happen, change nothing
#   -y, --yes           accept the selection and every prompt (non-interactive)
#   -h, --help          show this help
#
# Environment (mostly for testing and mirrors):
#   CUA_INSTALL_NONINTERACTIVE=1      never prompt (defaults plus --select)
#   CUA_INSTALL_REINSTALL=1           with --pair, install the CLI and app even if present
#   CUA_INSTALL_MANIFEST_URL  release-artifacts.json to read
#   CUA_INSTALL_BASE_URL      release download base (default GitHub releases)
#   CUA_INSTALL_DRIVER_URL    cua-driver installer (default cua.ai/driver/install.sh)
#   CUA_INSTALL_REPO          GitHub repo (default trycua/cua)
#   CUA_INSTALL_OS, CUA_INSTALL_ARCH  override platform detection
#   CUA_INSTALL_MINISIGN_PUBKEY       minisign public key for artifacts
#   CUA_HOME                  cua state dir (default ~/.cua)
set -eu

REPO="${CUA_INSTALL_REPO:-trycua/cua}"
BASE_URL="${CUA_INSTALL_BASE_URL:-https://github.com/$REPO/releases/download}"
# Rolling release that always carries the newest release-artifacts.json.
LATEST_TAG="cua-install-latest"
# Release key for artifact signatures (filled in by the release process).
MINISIGN_PUBKEY="${CUA_INSTALL_MINISIGN_PUBKEY:-}"
# Only the release workflows may sign, each at its own release tag: the CLI
# by cd-cua-sdk.yml at refs/tags/cua-sdk-v<version>, the app by
# cd-cua-spaces.yml at refs/tags/cua-spaces-v<version> (cosign_identity).
COSIGN_ISSUER="https://token.actions.githubusercontent.com"

want_cli=1
cli_only=0
app_only=0
select_list=""
only_list=""
only_set=0
sel_spaces=0
sel_driver=0
sel_host=0
picked=0
tty_saved=""
mode=""
pair_code=""
relay_url=""
version=""
prefix=""
modify_path=0
no_modify_path=0
sig_noted=0
onboarding=1
require_sig=0
dry_run=0
assume_yes=0
tmp=""
mounted=""
existing_cua=""

say() { printf '%s\n' "$*"; }
info() { printf 'cua-install: %s\n' "$*"; }
warn() { printf 'cua-install: warning: %s\n' "$*" >&2; }
die() {
    printf 'cua-install: error: %s\n' "$*" >&2
    exit 1
}

usage() {
    sed -n '2,/^set -eu/p' "$0" 2>/dev/null | sed -e '$d' -e 's/^# \{0,1\}//' || true
}

cleanup() {
    if [ -n "$tty_saved" ]; then
        stty "$tty_saved" </dev/tty 2>/dev/null || true
        tty_saved=""
    fi
    if [ -n "$mounted" ]; then
        hdiutil detach -quiet "$mounted" >/dev/null 2>&1 || true
    fi
    if [ -n "$tmp" ] && [ -d "$tmp" ]; then
        rm -rf "$tmp"
    fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# ---------------------------------------------------------------- arguments

parse_args() {
    while [ $# -gt 0 ]; do
        case "$1" in
        --cli-only) cli_only=1 ;;
        --app-only) app_only=1 ;;
        --select)
            [ $# -ge 2 ] || die "--select needs a list of items"
            select_list="$select_list,$2"
            shift
            ;;
        --select=*) select_list="$select_list,${1#--select=}" ;;
        --only)
            [ $# -ge 2 ] || die "--only needs a list of items"
            only_list="$only_list,$2"
            only_set=1
            shift
            ;;
        --only=*)
            only_list="$only_list,${1#--only=}"
            only_set=1
            ;;
        --mode)
            [ $# -ge 2 ] || die "--mode needs host or client"
            mode="$2"
            shift
            ;;
        --mode=*) mode="${1#--mode=}" ;;
        --pair)
            [ $# -ge 2 ] || die "--pair needs the code from the Cua Spaces app"
            pair_code="$2"
            shift
            ;;
        --pair=*) pair_code="${1#--pair=}" ;;
        --relay)
            [ $# -ge 2 ] || die "--relay needs a URL"
            relay_url="$2"
            shift
            ;;
        --relay=*) relay_url="${1#--relay=}" ;;
        --version)
            [ $# -ge 2 ] || die "--version needs a value"
            version="$2"
            shift
            ;;
        --version=*) version="${1#--version=}" ;;
        --prefix)
            [ $# -ge 2 ] || die "--prefix needs a directory"
            prefix="$2"
            shift
            ;;
        --prefix=*) prefix="${1#--prefix=}" ;;
        --modify-path) modify_path=1 ;;
        --no-modify-path) no_modify_path=1 ;;
        --no-onboarding) onboarding=0 ;;
        --require-signature) require_sig=1 ;;
        --dry-run) dry_run=1 ;;
        -y | --yes) assume_yes=1 ;;
        -h | --help)
            usage
            exit 0
            ;;
        *) die "unknown option: $1 (see --help)" ;;
        esac
        shift
    done
    if [ "$cli_only" = 1 ] && [ "$app_only" = 1 ]; then
        die "--cli-only and --app-only are mutually exclusive"
    fi
    if [ "$only_set" = 1 ] && [ -n "$select_list" ]; then
        die "--only and --select are mutually exclusive"
    fi
    extra="$(items_in "$select_list$only_list")"
    if [ "$cli_only" = 1 ] && [ -n "$extra" ] && [ -z "$pair_code" ]; then
        die "--cli-only installs only the CLI; use --only $extra instead"
    fi
    if [ "$app_only" = 1 ]; then
        case " $extra " in
        *" cua-driver "* | *" host "*) die "--app-only skips the CLI, which cua-driver and host need" ;;
        esac
        want_cli=0
    fi
    if [ -n "$pair_code" ]; then
        # Letters, digits and dashes only: it ends up on a command line.
        case "$pair_code" in
        *[!A-Za-z0-9-]*) die "--pair: '$pair_code' is not a pairing code (like K7QM-4XPB-2RHD)" ;;
        esac
        [ "$app_only" = 0 ] || die "--pair needs the CLI; drop --app-only"
        [ "$cli_only" = 0 ] || only_list="$only_list,host"
        select_list="$select_list,host"
        if [ "$only_set" = 1 ]; then only_list="$only_list,host"; fi
        # The code stands in for the account: no sign-in here.
        onboarding=0
    fi
    if [ -n "$relay_url" ]; then
        [ -n "$pair_code" ] || die "--relay goes with --pair"
        case "$relay_url" in
        *[!A-Za-z0-9:/._%-]*) die "--relay: '$relay_url' is not a plain URL" ;;
        esac
        # https anywhere; plain http only to this machine (local relays).
        printf '%s\n' "$relay_url" | grep -Eq '^(https://[A-Za-z0-9.-]+(:[0-9]+)?|http://(localhost|127\.0\.0\.1)(:[0-9]+)?)(/[A-Za-z0-9._%/-]*)?$' ||
            die "--relay must be an https:// URL (plain http only for localhost)"
    fi
    case "$mode" in
    "" | host | client) ;;
    *) die "--mode must be host or client, not '$mode'" ;;
    esac
    version="${version#v}"
    case "$version" in
    "" | [0-9]*.[0-9]*.[0-9]*) ;;
    *) die "--version must look like 1.2.3, not '$version'" ;;
    esac
    case "$prefix" in
    "" | /*) ;;
    *) prefix="$(pwd)/$prefix" ;;
    esac
}

# ---------------------------------------------------------------- helpers

has() { command -v "$1" >/dev/null 2>&1; }

# Whether a human can answer prompts (curl | sh keeps /dev/tty usable).
tty_ok() {
    [ -t 1 ] || [ -t 2 ] || return 1
    (: </dev/tty) 2>/dev/null
}

# confirm "question" default(y|n): 0 for yes. Answered once the checklist
# was confirmed.
confirm() {
    if [ "$assume_yes" = 1 ] || [ "$picked" = 1 ]; then
        return 0
    fi
    if ! tty_ok; then
        [ "$2" = y ]
        return
    fi
    if [ "$2" = y ]; then hint="[Y/n]"; else hint="[y/N]"; fi
    printf '%s %s ' "$1" "$hint" >/dev/tty
    answer=""
    read -r answer </dev/tty || answer=""
    case "$answer" in
    [yY] | [yY][eE][sS]) return 0 ;;
    [nN] | [nN][oO]) return 1 ;;
    *) [ "$2" = y ] ;;
    esac
}

# ---------------------------------------------------------------- selection

# items_in LIST: the item ids in a comma list (cli dropped), or an error.
items_in() {
    out=""
    for item in $(printf '%s' "$1" | tr ',' ' '); do
        case "$item" in
        cli) ;;
        spaces | cua-driver | host)
            case " $out " in *" $item "*) ;; *) out="${out:+$out }$item" ;; esac
            ;;
        *) die "unknown item '$item' (valid: cli, spaces, cua-driver, host)" ;;
        esac
    done
    printf '%s\n' "$out"
}

item_on() {
    case "$1" in
    cli) [ "$want_cli" = 1 ] ;;
    spaces) [ "$sel_spaces" = 1 ] ;;
    cua-driver) [ "$sel_driver" = 1 ] ;;
    host) [ "$sel_host" = 1 ] ;;
    esac
}

item_set() { # id 0|1
    case "$1" in
    spaces) sel_spaces=$2 ;;
    cua-driver) sel_driver=$2 ;;
    host) sel_host=$2 ;;
    esac
}

item_toggle() {
    if item_on "$1"; then item_set "$1" 0; else item_set "$1" 1; fi
}

item_label() {
    case "$1" in
    cli) printf 'cua CLI (required)' ;;
    spaces) printf 'Cua Spaces app' ;;
    cua-driver) printf 'cua-driver MCP and skill for your agents' ;;
    host) printf 'Host this machine' ;;
    esac
}

# The selection before the checklist: defaults, legacy flags, --select/--only.
init_selection() {
    if [ "$only_set" = 1 ] || [ "$cli_only" = 1 ]; then
        list="$only_list"
    else
        list="$select_list"
        # The app is a default on macOS and Linux.
        if spaces_supported; then sel_spaces=1; fi
    fi
    for item in $(items_in "$list"); do item_set "$item" 1; done
    # Legacy: --app-only and --mode mean the app, as they always did.
    if [ "$app_only" = 1 ] || { [ -n "$mode" ] && [ "$cli_only" = 0 ]; }; then
        sel_spaces=1
    fi
    if [ "$sel_spaces" = 1 ] && ! spaces_supported; then
        info "Cua Spaces is not available for $os; skipping the app"
        sel_spaces=0
    fi
}

# Cua Spaces ships for macOS (the SwiftUI app) and Linux (the Electron app,
# .deb or AppImage).
spaces_supported() {
    [ "$os" = darwin ] || [ "$os" = linux ]
}

# --pair on a computer that already has Cua: use the `cua` that is there
# (it must know `host setup --pair`) and keep the app that is there, instead
# of downloading both again. Anything else (--prefix, --version, an old cua,
# nothing installed) installs as before.
reuse_installed() {
    [ -n "$pair_code" ] || return 0
    [ -z "$prefix" ] && [ -z "$version" ] && [ "${CUA_INSTALL_REINSTALL:-0}" != 1 ] || return 0
    [ "$app_only" = 0 ] || return 0
    found=""
    for candidate in "$(command -v cua 2>/dev/null || true)" "$HOME/.local/bin/cua" /usr/local/bin/cua \
        "/Applications/Cua Spaces.app/Contents/MacOS/cua" "$HOME/Applications/Cua Spaces.app/Contents/MacOS/cua"; do
        [ -n "$candidate" ] && [ -x "$candidate" ] || continue
        if "$candidate" host setup --help 2>&1 </dev/null | grep -q -e '--pair'; then
            found="$candidate"
            break
        fi
    done
    [ -n "$found" ] || return 0
    info "cua is already installed at $found; using it (CUA_INSTALL_REINSTALL=1 installs it again)"
    existing_cua="$found"
    want_cli=0
    case "$os" in
    darwin)
        if [ -d "/Applications/Cua Spaces.app" ] || [ -d "$HOME/Applications/Cua Spaces.app" ]; then sel_spaces=0; fi
        ;;
    linux)
        if has cua-spaces || [ -x "$HOME/.local/bin/cua-spaces" ]; then sel_spaces=0; fi
        ;;
    esac
    if [ "$sel_spaces" = 0 ] && spaces_supported; then info "Cua Spaces is already installed; keeping it"; fi
}

# Show the checklist only when a human can answer it.
wants_checklist() {
    [ -z "$pair_code" ] && [ "$assume_yes" = 0 ] && [ "$only_set" = 0 ] && [ "$cli_only" = 0 ] &&
        [ "$app_only" = 0 ] && [ "${CUA_INSTALL_NONINTERACTIVE:-}" != 1 ] && tty_ok
}

row_id() { # N: the item on row N
    i=0
    for r in $rows; do
        i=$((i + 1))
        if [ "$i" = "$1" ]; then printf '%s' "$r"; fi
    done
}

draw_rows() { # cursor row, or 0 for the numbered list
    i=0
    for r in $rows; do
        i=$((i + 1))
        if item_on "$r"; then box='[x]'; else box='[ ]'; fi
        if [ "$1" = 0 ]; then
            printf '  %d %s %s\n' "$i" "$box" "$(item_label "$r")"
        else
            if [ "$i" = "$1" ]; then p='>'; else p=' '; fi
            printf '\r\033[K%s %s %s\n' "$p" "$box" "$(item_label "$r")"
        fi
    done
}

# checklist: pick items with arrows/numbers + space, Enter to install.
checklist() {
    rows="cli"
    # Spaces is offered only where it ships (macOS, Linux).
    if spaces_supported; then rows="$rows spaces"; fi
    rows="$rows cua-driver host"
    n=0
    for r in $rows; do n=$((n + 1)); done
    printf '\nChoose what to install:\n' >/dev/tty
    if [ "${TERM:-dumb}" = dumb ] || ! tty_saved="$(stty -g </dev/tty 2>/dev/null)"; then
        tty_saved=""
        while :; do
            draw_rows 0 >/dev/tty
            printf 'Toggle numbers (e.g. 2 3), Enter to install: ' >/dev/tty
            line=""
            read -r line </dev/tty || line=""
            [ -n "$line" ] || break
            for k in $(printf '%s' "$line" | tr ',' ' '); do
                id="$(row_id "$k")"
                [ -n "$id" ] && item_toggle "$id"
            done
        done
    else
        stty -icanon -echo min 1 time 0 </dev/tty
        esc="$(printf '\033')"
        cr="$(printf '\r')"
        cur=1
        draw_rows "$cur" >/dev/tty
        printf 'Up/down or 1-%d to move/toggle, space to toggle, Enter to install\n' "$n" >/dev/tty
        while :; do
            key="$(dd bs=1 count=1 2>/dev/null </dev/tty)" || key=""
            case "$key" in
            "" | "$cr" | "$(printf '\004')") break ;; # Enter, EOF or Ctrl-D
            "$esc")
                key="$(dd bs=1 count=2 2>/dev/null </dev/tty)" || key=""
                case "$key" in
                ?A) [ "$cur" -gt 1 ] && cur=$((cur - 1)) ;;
                ?B) [ "$cur" -lt "$n" ] && cur=$((cur + 1)) ;;
                esac
                ;;
            k) [ "$cur" -gt 1 ] && cur=$((cur - 1)) ;;
            j) [ "$cur" -lt "$n" ] && cur=$((cur + 1)) ;;
            " ") item_toggle "$(row_id "$cur")" ;;
            [1-9])
                if [ "$key" -le "$n" ]; then
                    cur=$key
                    item_toggle "$(row_id "$cur")"
                fi
                ;;
            esac
            printf '\033[%dA' "$((n + 1))" >/dev/tty
            draw_rows "$cur" >/dev/tty
            printf '\n' >/dev/tty
        done
        stty "$tty_saved" </dev/tty 2>/dev/null || true
        tty_saved=""
    fi
    printf '\n' >/dev/tty
    picked=1
}

run() {
    if [ "$dry_run" = 1 ]; then
        say "[dry-run] $*"
    else
        "$@"
    fi
}

detect_platform() {
    os="${CUA_INSTALL_OS:-$(uname -s)}"
    arch="${CUA_INSTALL_ARCH:-$(uname -m)}"
    case "$os" in
    Darwin | darwin | macos) os=darwin ;;
    Linux | linux) os=linux ;;
    MINGW* | MSYS* | CYGWIN* | Windows_NT | windows)
        die "on Windows, run: irm https://cua.ai/install.ps1 | iex"
        ;;
    *) die "unsupported OS: $os" ;;
    esac
    case "$arch" in
    x86_64 | amd64 | x64) arch=x64 ;;
    arm64 | aarch64 | armv8*) arch=arm64 ;;
    *) die "unsupported CPU architecture: $arch" ;;
    esac
    # A shell under Rosetta reports x86_64 on Apple silicon.
    if [ "$os" = darwin ] && [ "$arch" = x64 ] && [ -z "${CUA_INSTALL_ARCH:-}" ]; then
        if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
            arch=arm64
        fi
    fi
    if [ "$os" = linux ] && [ -z "${CUA_INSTALL_OS:-}" ] && has ldd; then
        if ldd --version 2>&1 | grep -qi musl; then
            warn "musl libc detected; the cua CLI needs glibc 2.31 or newer"
        fi
    fi
    platform="$os-$arch"
}

# check_url URL: HTTPS only. Plain HTTP is allowed only to a loopback host
# (tests and local mirrors), file:// only for a local manifest.
check_url() {
    case "$1" in
    *[[:space:]]* | http://*@*) die "refusing a malformed download URL: $1" ;;
    https://*) ;;
    http://127.0.0.1[:/]* | http://localhost[:/]* | http://\[::1\][:/]*) ;;
    file://*)
        case "${manifest_url:-}" in
        file://*) ;;
        *) die "refusing a file:// URL from a remote manifest: $1" ;;
        esac
        ;;
    *) die "refusing a non-HTTPS download: $1" ;;
    esac
}

# fetch URL DEST
fetch() {
    check_url "$1"
    case "$1" in
    file://*)
        cp "${1#file://}" "$2" || die "could not read $1"
        return
        ;;
    esac
    if has curl; then
        # Redirects stay on HTTPS unless the URL itself is a loopback test server.
        case "$1" in
        https://*) proto='=https' ;;
        *) proto='=https,http' ;;
        esac
        curl --proto "$proto" --proto-redir "$proto" --tlsv1.2 -fsSL --retry 3 -o "$2" "$1" || die "download failed: $1"
    elif has wget; then
        case "$1" in
        https://*) wget -q --https-only -O "$2" "$1" || die "download failed: $1" ;;
        *) wget -q -O "$2" "$1" || die "download failed: $1" ;;
        esac
    else
        die "need curl or wget to download"
    fi
}

sha256_of() {
    if has sha256sum; then
        sha256sum "$1" | cut -d' ' -f1
    elif has shasum; then
        shasum -a 256 "$1" | cut -d' ' -f1
    elif has openssl; then
        openssl dgst -sha256 "$1" | sed 's/.*= *//'
    else
        die "need sha256sum, shasum or openssl to verify downloads"
    fi
}

# json_field LINE KEY: a string value from one manifest line.
json_field() {
    printf '%s\n' "$1" | sed -n "s/.*\"$2\" *: *\"\\([^\"]*\\)\".*/\\1/p"
}

# manifest_entry COMPONENT KIND: the artifact line for this platform.
manifest_entry() {
    grep "\"component\" *: *\"$1\"" "$manifest" |
        grep "\"platform\" *: *\"$platform\"" |
        grep "\"kind\" *: *\"$2\"" | head -n 1 || true
}

load_manifest() {
    if [ -n "${CUA_INSTALL_MANIFEST_URL:-}" ]; then
        manifest_url="$CUA_INSTALL_MANIFEST_URL"
    elif [ -n "$version" ]; then
        manifest_url="$BASE_URL/cua-sdk-v$version/release-artifacts.json"
    else
        manifest_url="$BASE_URL/$LATEST_TAG/release-artifacts.json"
    fi
    manifest="$tmp/release-artifacts.json"
    fetch "$manifest_url" "$manifest"
    grep -q '"schema" *: *1' "$manifest" || die "unsupported release manifest at $manifest_url"
    manifest_dir="${manifest_url%/*}"
}

# resolve_url VALUE: absolute URLs pass through; names are relative to the manifest.
resolve_url() {
    case "$1" in
    *://*) printf '%s\n' "$1" ;;
    *) printf '%s/%s\n' "$manifest_dir" "$1" ;;
    esac
}

# cosign_identity ENTRY: the exact Sigstore certificate identity (workflow
# at its release tag) that must have signed this manifest entry's artifact.
cosign_identity() {
    c="$(json_field "$1" component)"
    v="$(json_field "$1" version)"
    case "$v" in
    "" | *[!0-9A-Za-z.+-]*) return 1 ;;
    esac
    case "$c" in
    cli) printf 'https://github.com/%s/.github/workflows/cd-cua-sdk.yml@refs/tags/cua-sdk-v%s\n' "$REPO" "$v" ;;
    app) printf 'https://github.com/%s/.github/workflows/cd-cua-spaces.yml@refs/tags/cua-spaces-v%s\n' "$REPO" "$v" ;;
    *) return 1 ;;
    esac
}

verify_signature() { # file entry
    sig_ok=0
    minisig="$(json_field "$2" minisig)"
    if [ -n "$minisig" ] && [ -n "$MINISIGN_PUBKEY" ] && has minisign; then
        fetch "$(resolve_url "$minisig")" "$1.minisig"
        minisign -Vqm "$1" -x "$1.minisig" -P "$MINISIGN_PUBKEY" >/dev/null ||
            die "minisign signature check failed for ${1##*/}"
        sig_ok=1
    fi
    bundle="$(json_field "$2" cosign_bundle)"
    if [ "$sig_ok" = 0 ] && [ -n "$bundle" ] && has cosign; then
        fetch "$(resolve_url "$bundle")" "$1.sigstore.json"
        identity="$(cosign_identity "$2")" ||
            die "no release identity for ${1##*/} in the manifest"
        cosign verify-blob --bundle "$1.sigstore.json" \
            --certificate-identity "$identity" \
            --certificate-oidc-issuer "$COSIGN_ISSUER" "$1" >/dev/null 2>&1 ||
            die "cosign signature check failed for ${1##*/}"
        sig_ok=1
    fi
    if [ "$sig_ok" = 0 ] && [ "$require_sig" = 1 ]; then
        die "no verifiable signature for ${1##*/} (install minisign or cosign)"
    fi
    if [ "$sig_ok" = 0 ] && { [ -n "$minisig" ] || [ -n "$bundle" ]; }; then
        # One quiet line per run: sha256 is checked either way.
        if [ "$sig_noted" = 0 ]; then
            sig_noted=1
            info "checked sha256 (cosign is not installed, so release signatures were not checked; --require-signature makes that an error)"
        fi
    fi
}

# download COMPONENT KIND: sets $artifact to a verified local file.
download() {
    entry="$(manifest_entry "$1" "$2")"
    [ -n "$entry" ] || die "no $1 $2 artifact for $platform in the release manifest"
    name="$(json_field "$entry" name)"
    url="$(resolve_url "$(json_field "$entry" url)")"
    sum="$(json_field "$entry" sha256)"
    [ -n "$name" ] && [ -n "$sum" ] || die "incomplete manifest entry for $1 $2 on $platform"
    case "$name" in
    */* | .* | "") die "bad artifact name in manifest: $name" ;;
    esac
    if [ "$dry_run" = 1 ]; then
        say "[dry-run] download $url (sha256 $sum)"
        artifact="$tmp/$name"
        return
    fi
    info "downloading $name"
    artifact="$tmp/$name"
    fetch "$url" "$artifact"
    actual="$(sha256_of "$artifact")"
    if [ "$actual" != "$sum" ]; then
        die "checksum mismatch for $name (expected $sum, got $actual); aborting"
    fi
    verify_signature "$artifact" "$entry"
}

# ---------------------------------------------------------------- the CLI

cli_bin_dir() {
    if [ -n "$prefix" ]; then
        printf '%s/bin\n' "$prefix"
    elif [ "$(id -u)" = 0 ]; then
        printf '/usr/local/bin\n'
    else
        printf '%s/.local/bin\n' "$HOME"
    fi
}

path_has() {
    case ":$PATH:" in
    *":$1:"*) return 0 ;;
    *) return 1 ;;
    esac
}

profile_file() {
    case "${SHELL:-}" in
    */zsh) printf '%s/.zshrc\n' "${ZDOTDIR:-$HOME}" ;;
    */bash)
        if [ "$os" = darwin ]; then printf '%s/.bash_profile\n' "$HOME"; else printf '%s/.bashrc\n' "$HOME"; fi
        ;;
    */fish) printf '%s/fish/config.fish\n' "${XDG_CONFIG_HOME:-$HOME/.config}" ;;
    *) printf '%s/.profile\n' "$HOME" ;;
    esac
}

path_line() { # dir profile
    case "$2" in
    *.fish) printf 'fish_add_path "%s"\n' "$1" ;;
    *)
        # shellcheck disable=SC2016 # $PATH is expanded by the profile, not here.
        printf 'export PATH="%s:$PATH"\n' "$1"
        ;;
    esac
}

ensure_path() { # dir
    if path_has "$1"; then
        return
    fi
    profile="$(profile_file)"
    line="$(path_line "$1" "$profile")"
    if [ "$modify_path" = 0 ] && [ "$no_modify_path" = 0 ] && [ "$assume_yes" = 0 ] && [ "$dry_run" = 0 ] && [ "${CUA_INSTALL_NONINTERACTIVE:-0}" != 1 ] && tty_ok; then
        # Interactive default: offer to add it (Enter = yes).
        printf 'Add %s to PATH in %s so cua works in new terminals? [Y/n] ' "$1" "$profile" >/dev/tty
        answer=""
        read -r answer </dev/tty || answer=""
        case "$answer" in
        [nN] | [nN][oO]) ;;
        *) modify_path=1 ;;
        esac
    fi
    if [ "$modify_path" = 1 ] && [ "$no_modify_path" = 0 ]; then
        if [ -f "$profile" ] && grep -qF "$line" "$profile"; then
            return
        fi
        if [ "$dry_run" = 1 ]; then
            say "[dry-run] append PATH line to $profile"
            return
        fi
        mkdir -p "$(dirname "$profile")"
        printf '\n# Added by the cua installer\n%s\n' "$line" >>"$profile"
        info "added $1 to PATH in $profile (open a new terminal to pick it up)"
    else
        say ""
        say "$1 is not on your PATH. Add it with this one line, then open a new terminal:"
        say "  echo '$line' >> $profile"
        say "(or rerun the installer with --modify-path)"
    fi
}

install_cli() {
    bindir="$(cli_bin_dir)"
    target="$bindir/cua"
    if ! confirm "Install the cua CLI to $target?" y; then
        die "cancelled; rerun with --prefix DIR to choose another location"
    fi
    download cli tar.gz
    if [ "$dry_run" = 1 ]; then
        say "[dry-run] install cua -> $target"
    else
        mkdir -p "$tmp/cli"
        tar -xzf "$artifact" -C "$tmp/cli" || die "could not unpack ${artifact##*/}"
        src="$(find "$tmp/cli" -type f -name cua | head -n 1)"
        [ -n "$src" ] || die "no cua binary inside ${artifact##*/}"
        mkdir -p "$bindir" || die "cannot create $bindir (try --prefix or run as root)"
        cp "$src" "$bindir/.cua.tmp.$$"
        chmod 0755 "$bindir/.cua.tmp.$$"
        mv -f "$bindir/.cua.tmp.$$" "$target"
        info "installed $target"
    fi
    cua_bin="$target"
    ensure_path "$bindir"
}

# ---------------------------------------------------------------- the app

install_app_darwin() {
    # The macOS app (SwiftUI) needs macOS 26; an older Mac would get an app
    # that cannot open.
    if [ -z "${CUA_INSTALL_OS:-}" ] && has sw_vers; then
        macos="$(sw_vers -productVersion 2>/dev/null || true)"
        case "${macos%%.*}" in
        '' | *[!0-9]*) ;;
        *)
            if [ "${macos%%.*}" -lt 26 ]; then
                warn "Cua Spaces for macOS needs macOS 26 or later (this Mac runs $macos); skipping the app"
                return
            fi
            ;;
        esac
    fi
    if [ -n "$prefix" ]; then
        appdir="$prefix/Applications"
    elif [ -w /Applications ]; then
        appdir="/Applications"
    else
        appdir="$HOME/Applications"
    fi
    if ! confirm "Install Cua Spaces to $appdir?" y; then
        if [ "$appdir" = /Applications ] && confirm "Install to $HOME/Applications instead?" y; then
            appdir="$HOME/Applications"
        else
            info "skipping the Cua Spaces app"
            return
        fi
    fi
    download app dmg
    if [ "$dry_run" = 1 ]; then
        say "[dry-run] mount ${artifact##*/} and copy Cua Spaces.app -> $appdir"
        app_path="$appdir/Cua Spaces.app"
        return
    fi
    has hdiutil || die "hdiutil not found; cannot mount the disk image"
    mounted="$tmp/mnt"
    mkdir -p "$mounted"
    hdiutil attach -nobrowse -readonly -noautoopen -quiet -mountpoint "$mounted" "$artifact" ||
        {
            mounted=""
            die "could not mount ${artifact##*/}"
        }
    src="$(find "$mounted" -maxdepth 1 -name '*.app' | head -n 1)"
    [ -n "$src" ] || die "no .app in ${artifact##*/}"
    app_path="$appdir/${src##*/}"
    mkdir -p "$appdir"
    rm -rf "$app_path.cua-new"
    ditto "$src" "$app_path.cua-new" 2>/dev/null || cp -R "$src" "$app_path.cua-new"
    rm -rf "$app_path"
    mv "$app_path.cua-new" "$app_path"
    hdiutil detach -quiet "$mounted" >/dev/null 2>&1 || true
    mounted=""
    info "installed $app_path"
}

install_app_linux() {
    deb_entry="$(manifest_entry app deb)"
    # Debian and Ubuntu: the .deb (apt resolves its dependencies), as root or
    # with passwordless sudo. Anything else, or no privileges: the AppImage.
    if [ -z "$prefix" ] && has apt-get && [ -n "$deb_entry" ]; then
        sudo_cmd=""
        if [ "$(id -u)" != 0 ]; then
            if has sudo && sudo -n true 2>/dev/null; then sudo_cmd="sudo"; else sudo_cmd="none"; fi
        fi
        if [ "$sudo_cmd" != none ] && confirm "Install the Cua Spaces .deb with apt-get?" y; then
            download app deb
            # shellcheck disable=SC2086
            run $sudo_cmd apt-get install -y "$artifact"
            app_path="cua-spaces"
            return
        fi
    fi
    bindir="$(cli_bin_dir)"
    target="$bindir/cua-spaces"
    if ! confirm "Install the Cua Spaces AppImage to $target?" y; then
        info "skipping the Cua Spaces app"
        return
    fi
    download app appimage
    app_path="$target"
    if [ -n "$prefix" ]; then
        desktop_dir="$prefix/share/applications"
    else
        desktop_dir="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
    fi
    if [ "$dry_run" = 1 ]; then
        say "[dry-run] install AppImage -> $target"
        say "[dry-run] write $desktop_dir/cua-spaces.desktop"
        return
    fi
    mkdir -p "$bindir" "$desktop_dir"
    cp "$artifact" "$bindir/.cua-spaces.tmp.$$"
    chmod 0755 "$bindir/.cua-spaces.tmp.$$"
    mv -f "$bindir/.cua-spaces.tmp.$$" "$target"
    cat >"$desktop_dir/cua-spaces.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Cua Spaces
Comment=Reach your cloud Spaces and your own machines
Exec="$target" %U
Terminal=false
Categories=Development;
EOF
    info "installed $target"
}

write_mode() {
    [ -n "$mode" ] || return 0
    home="${CUA_HOME:-$HOME/.cua}"
    if [ "$dry_run" = 1 ]; then
        say "[dry-run] write $home/spaces-install-mode ($mode)"
        return
    fi
    mkdir -p "$home"
    printf '%s\n' "$mode" >"$home/spaces-install-mode"
}

# How Cua was installed, for the one-time first-run telemetry event (a fixed
# enum value; no identifiers). Kept when another installer recorded one.
record_install_channel() {
    home="${CUA_HOME:-$HOME/.cua}"
    file="$home/telemetry/install_channel"
    if [ "$dry_run" = 1 ]; then
        say "[dry-run] record the install channel in $file"
        return
    fi
    [ -e "$file" ] && return 0
    mkdir -p "$home/telemetry" 2>/dev/null || return 0
    printf 'install_script\n' >"$file" 2>/dev/null || true
}

# ---------------------------------------------------------------- cua-driver

# cua-driver's own release installer (it verifies each archive against the
# release's SHA256SUMS and its Sigstore bundle), then the skill + MCP server.
install_driver() {
    if [ -n "$prefix" ]; then
        driver_dir="$prefix/bin"
        set -- --bin-dir "$driver_dir"
    else
        driver_dir="${CUA_DRIVER_RS_INSTALL_DIR:-$HOME/.local/bin}"
        set --
    fi
    [ "$modify_path" = 1 ] || set -- "$@" --no-modify-path
    [ "$require_sig" = 1 ] && set -- "$@" --require-signature
    script="$tmp/cua-driver-install.sh"
    if [ "$dry_run" = 1 ]; then
        say "[dry-run] download $driver_url"
        say "[dry-run] bash ${script##*/} $*"
    else
        info "installing cua-driver"
        fetch "$driver_url" "$script"
        bash "$script" "$@" </dev/null || die "the cua-driver installer failed"
    fi
    info "adding the cua-driver skill and MCP server to your agents"
    set -- "$cua_bin" agents setup --cua-driver --agents all --yes
    if [ "$dry_run" = 1 ] || [ -x "$driver_dir/cua-driver" ]; then
        set -- "$@" --mcp-command "$driver_dir/cua-driver"
    fi
    if [ "$dry_run" = 1 ]; then
        say "[dry-run] $*"
    elif ! "$@" </dev/null; then
        warn "agent setup for cua-driver did not finish; rerun: cua agents setup --cua-driver"
    fi
}

# Host this machine: joins the relay as the signed-in account. `cua host
# setup` installs cua-spacesd itself; a spare machine that should also
# provide Spaces for other devices (macOS VMs) needs Lume too, installed
# separately with `cua runtime setup lume`.
host_setup() {
    if [ -n "$pair_code" ]; then
        host_pair
        return
    fi
    if [ "$dry_run" = 1 ]; then
        say "[dry-run] $cua_bin host setup"
        return
    fi
    info "setting up this machine as a host (cua host setup)"
    if tty_ok; then
        if "$cua_bin" host setup </dev/tty; then
            say "  cua runtime setup lume  # also needed to provide macOS VM Spaces (cua host setup --profile spare --provide-spaces)"
            return
        fi
    else
        if "$cua_bin" host setup </dev/null; then
            say "  cua runtime setup lume  # also needed to provide macOS VM Spaces (cua host setup --profile spare --provide-spaces)"
            return
        fi
    fi
    warn "host setup did not finish; run 'cua auth login --remote' (over ssh) or 'cua auth login', then 'cua host setup' again"
}

# Add this machine with a pairing code (Add a machine in the app): joins
# the account (or team) that made the code, with no sign-in here.
host_pair() {
    set -- "$cua_bin" host setup --pair "$pair_code"
    if [ -n "$relay_url" ]; then set -- "$@" --relay "$relay_url"; fi
    if [ "$dry_run" = 1 ]; then
        say "[dry-run] $*"
        return
    fi
    # A CLI released before pairing exists would fail with a bare
    # "unexpected argument '--pair'"; say what to do instead.
    if ! "$cua_bin" host setup --help 2>&1 </dev/null | grep -q -e '--pair'; then
        die "the installed cua CLI ($("$cua_bin" --version 2>/dev/null || echo unknown)) cannot add a machine with a code (no 'cua host setup --pair'); update the CLI (cua update, or install a newer release: --version) or sign in instead: cua auth login && cua host setup"
    fi
    info "adding this machine to your account (cua host setup --pair)"
    if "$@" </dev/null; then
        say ""
        say "This machine is added. It shows up in Cua Spaces on your other devices in a few seconds."
        return
    fi
    die "adding this machine did not finish; if the code expired or was used, make a new one in Cua Spaces (Add a machine) and run: cua host setup --pair <code>"
}

# ---------------------------------------------------------------- onboarding

next_steps() {
    say ""
    say "Next steps:"
    if [ "$want_cli" = 1 ]; then
        say "  cua auth login      # sign in (over ssh / no browser: --remote), then set up your AI coding agents"
        say "  cua agents setup    # (re)configure cua skills and the cua MCP server"
    fi
    if [ "$sel_host" = 1 ]; then
        say "  cua host setup       # hosting Spaces needs you signed in first"
    fi
    if [ "$sel_spaces" = 1 ] && [ -n "${app_path:-}" ]; then
        if [ "$os" = darwin ]; then
            say "  open \"$app_path\"  # start Cua Spaces"
        else
            say "  $app_path  # start Cua Spaces"
        fi
    fi
}

run_onboarding() {
    # --pair: the code stands in for the account; nothing to sign in to.
    [ -z "$pair_code" ] || return 0
    if [ "$want_cli" = 0 ] || [ "$onboarding" = 0 ] || [ "$dry_run" = 1 ]; then
        next_steps
        return
    fi
    if ! tty_ok; then
        # No terminal to sign in on (piped install, -y, or nobody at the
        # console over ssh): `cua auth login` needs one, so it was skipped.
        # Tell the user exactly how to finish instead of leaving this
        # silent; hosting Spaces (`cua host setup`) needs a signed-in
        # account too.
        say ""
        info "not signing in: no terminal to prompt on"
        say "Sign in from here with a device code (works over ssh, no browser needed):"
        say "  cua auth login --remote"
        next_steps
        return
    fi
    say ""
    info "signing in (cua auth login)"
    # `cua auth login` runs agent onboarding after a successful sign-in.
    if ! "$cua_bin" auth login </dev/tty; then
        warn "sign-in did not finish; run 'cua auth login' later (or 'cua auth login --remote' over ssh)"
        next_steps
    fi
}

main() {
    parse_args "$@"
    detect_platform
    init_selection
    reuse_installed
    tmp="$(mktemp -d 2>/dev/null || mktemp -d -t cua-install)"
    if wants_checklist; then
        checklist
    fi
    if [ "$dry_run" = 1 ]; then
        info "dry run: nothing will be changed"
    fi
    # Nothing to download (--pair on a computer that has Cua): no manifest to read.
    if [ "$want_cli" = 1 ] || [ "$sel_spaces" = 1 ] || [ "$sel_driver" = 1 ]; then
        load_manifest
        info "platform $platform, manifest $manifest_url"
    fi
    # Fail before installing anything when cua-driver cannot be installed.
    driver_url="${CUA_INSTALL_DRIVER_URL:-https://cua.ai/driver/install.sh}"
    if [ "$sel_driver" = 1 ]; then
        check_url "$driver_url"
        [ "$dry_run" = 1 ] || has bash || die "cua-driver: its installer needs bash; install bash and rerun"
    fi
    cua_bin="${existing_cua:-cua}"
    app_path=""
    if [ "$want_cli" = 1 ]; then
        install_cli
    fi
    if [ "$sel_spaces" = 1 ]; then
        case "$os" in
        darwin) install_app_darwin ;;
        linux) install_app_linux ;;
        esac
        write_mode
    fi
    record_install_channel
    # The CLI invoked here reports the channel even before the file is read.
    CUA_INSTALL_CHANNEL=install_script
    export CUA_INSTALL_CHANNEL
    if [ "$sel_driver" = 1 ]; then
        install_driver
    fi
    info "done"
    run_onboarding
    if [ "$sel_host" = 1 ]; then
        host_setup
    fi
}

main "$@"
