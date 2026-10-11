#!/bin/sh
# Plain-sh tests for install.sh against a local HTTP server serving a fake
# release. Hermetic: fake HOME, --prefix / temp dirs, fake `cua` binaries.
# Nothing touches the real home directory, /Applications or PATH.
#
#   scripts/install/tests/run.sh            # all tests
#   CUA_INSTALL_TEST_SHELL=dash run.sh      # run install.sh under another sh
#
# Needs python3 (HTTP server + manifest generator), tar, curl or wget.
# The macOS .dmg test runs only on macOS (hdiutil, temp mount point).
set -u

here="$(cd "$(dirname "$0")" && pwd)"
installer="$here/../install.sh"
manifest_tool="$here/../release_manifest.py"
sh_bin="${CUA_INSTALL_TEST_SHELL:-sh}"
work="$(mktemp -d 2>/dev/null || mktemp -d -t cua-install-tests)"
server_pid=""
pass=0
fail=0

cleanup() {
    [ -n "$server_pid" ] && kill "$server_pid" 2>/dev/null
    rm -rf "$work"
}
trap cleanup EXIT INT TERM

ok() {
    pass=$((pass + 1))
    printf 'ok   %s\n' "$1"
}
not_ok() {
    fail=$((fail + 1))
    printf 'FAIL %s\n' "$1"
    [ -f "$work/out" ] && sed 's/^/     | /' "$work/out"
}
check() { # name command...
    name="$1"
    shift
    if "$@"; then ok "$name"; else not_ok "$name"; fi
}

# ------------------------------------------------------------ fake release

version=1.2.3
release="$work/release"
tag_dir="$release/cua-sdk-v$version"
latest_dir="$release/cua-install-latest"
mkdir -p "$tag_dir" "$latest_dir" "$work/stage"

# A fake `cua` that records its arguments.
make_cua() { # dir platform
    mkdir -p "$1"
    cat >"$1/cua" <<EOF
#!/bin/sh
if [ "\${1:-}" = --version ]; then echo "cua $version ($2)"; exit 0; fi
echo "\$*" >>"\${CUA_FAKE_LOG:-/dev/null}"
EOF
    chmod +x "$1/cua"
}

for platform in linux-x64 linux-arm64 darwin-arm64 darwin-x64; do
    make_cua "$work/stage/$platform" "$platform"
    tar -czf "$tag_dir/cua-cli-$version-$platform.tar.gz" -C "$work/stage/$platform" cua
done
for arch in x64 arm64; do
    printf '#!/bin/sh\necho cua-spaces %s\n' "$arch" >"$tag_dir/cua-spaces-$version-linux-$arch.AppImage"
    printf 'not really a deb\n' >"$tag_dir/cua-spaces-$version-linux-$arch.deb"
done

if [ "$(uname -s)" = Darwin ]; then
    mkdir -p "$work/dmgsrc/Cua Spaces.app/Contents/MacOS"
    printf '#!/bin/sh\n' >"$work/dmgsrc/Cua Spaces.app/Contents/MacOS/cua-spaces"
    hdiutil create -quiet -volname "Cua Spaces" -srcfolder "$work/dmgsrc" -ov -format UDZO \
        "$tag_dir/cua-spaces-$version-darwin-universal.dmg" || echo "hdiutil create failed" >&2
else
    printf 'fake dmg\n' >"$tag_dir/cua-spaces-$version-darwin-universal.dmg"
fi

python3 "$manifest_tool" --component cli --version "$version" --dir "$tag_dir" \
    --out "$work/cli.json" || exit 1
python3 "$manifest_tool" --component app --version "$version" --dir "$tag_dir" \
    --merge "$work/cli.json" --out "$tag_dir/release-artifacts.json" || exit 1
port_file="$work/port"
(cd "$release" && exec python3 -c '
import http.server, socketserver, sys
class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *a): pass
with socketserver.TCPServer(("127.0.0.1", 0), Quiet) as s:
    open(sys.argv[1], "w").write(str(s.server_address[1]))
    s.serve_forever()
' "$port_file" >/dev/null 2>&1) &
server_pid=$!
i=0
while [ ! -s "$port_file" ] && [ $i -lt 100 ]; do
    sleep 0.1
    i=$((i + 1))
done
[ -s "$port_file" ] || {
    echo "HTTP server did not start" >&2
    exit 1
}
base="http://127.0.0.1:$(cat "$port_file")"
python3 "$manifest_tool" --component cli --version "$version" --dir "$tag_dir" \
    --base-url "$base/cua-sdk-v$version" --out "$work/cli-abs.json" || exit 1
python3 "$manifest_tool" --component app --version "$version" --dir "$tag_dir" \
    --base-url "$base/cua-sdk-v$version" --merge "$work/cli-abs.json" \
    --out "$latest_dir/release-artifacts.json" || exit 1

# A fake cua-driver installer: logs its args, drops cua-driver in --bin-dir.
mkdir -p "$release/driver"
cat >"$release/driver/install.sh" <<'EOF'
#!/bin/bash
printf '%s\n' "$*" >>"$HOME/driver.log"
bin="$HOME/.local/bin"
while [ $# -gt 0 ]; do
    case "$1" in --bin-dir) bin="$2"; shift ;; esac
    shift
done
mkdir -p "$bin"
printf '#!/bin/sh\necho cua-driver\n' >"$bin/cua-driver"
chmod +x "$bin/cua-driver"
EOF
driver_url="$base/driver/install.sh"

# ------------------------------------------------------------ harness

# inst [ENV=VAL ...] -- ARGS: run install.sh with a fresh fake HOME.
inst() {
    home="$work/home.$$.$(date +%s%N 2>/dev/null || date +%s).$RANDOM_SEQ"
    RANDOM_SEQ=$((RANDOM_SEQ + 1))
    mkdir -p "$home"
    envs=""
    while [ $# -gt 0 ] && [ "$1" != -- ]; do
        envs="$envs $1"
        shift
    done
    [ "${1:-}" = -- ] && shift
    # shellcheck disable=SC2086
    env -i PATH="$PATH" HOME="$home" SHELL=/bin/sh CUA_HOME="$home/.cua" \
        CUA_INSTALL_BASE_URL="$base" CUA_INSTALL_OS=linux CUA_INSTALL_ARCH=x86_64 \
        CUA_FAKE_LOG="$home/cua.log" $envs \
        "$sh_bin" "$installer" "$@" </dev/null >"$work/out" 2>&1
    status=$?
    return 0
}
RANDOM_SEQ=0
has_out() { grep -q "$1" "$work/out"; }
exists() { [ -e "$1" ]; }
absent() { [ ! -e "$1" ]; }

# ------------------------------------------------------------ tests

inst -- --yes --no-onboarding --prefix "$work/p1"
check "linux x64: exits 0" [ "$status" = 0 ]
check "linux x64: installs cua under --prefix/bin" exists "$work/p1/bin/cua"
check "linux x64: installed cua runs and is the x64 build" sh -c "'$work/p1/bin/cua' --version | grep -q linux-x64"
check "linux default: no AppImage" absent "$work/p1/bin/cua-spaces"
check "linux default: no desktop entry" absent "$work/p1/share/applications/cua-spaces.desktop"
check "linux default: no cua-driver" absent "$home/driver.log"
check "linux x64: uses the rolling latest manifest" has_out "cua-install-latest/release-artifacts.json"
check "non-interactive: prints next steps, no login" sh -c "grep -q 'cua auth login' '$work/out' && [ ! -e '$home/cua.log' ]"
check "prefix bin not on PATH: prints a hint" has_out "is not on your PATH"
check "no --modify-path: profile untouched" absent "$home/.profile"

# ------------------------------------------------------------ selection

# Cua Spaces ships for macOS only for now: Linux skips it with a note (the
# manifest still lists Linux app artifacts; they are never downloaded).
inst -- --yes --no-onboarding --select spaces --prefix "$work/s1"
check "--select spaces (Linux): skips the app with a note" sh -c "[ $status = 0 ] && grep -q 'Cua Spaces is macOS-only for now' '$work/out' && ! grep -q 'cua-spaces-$version-linux' '$work/out'"
check "--select spaces (Linux): no AppImage" absent "$work/s1/bin/cua-spaces"
check "--select spaces (Linux): no desktop entry" absent "$work/s1/share/applications/cua-spaces.desktop"
check "--select spaces (Linux): still installs the CLI" exists "$work/s1/bin/cua"

inst -- --no-onboarding --only spaces --prefix "$work/s2"
check "--only spaces (Linux): installs the CLI, skips the app" sh -c "[ $status = 0 ] && [ -e '$work/s2/bin/cua' ] && [ ! -e '$work/s2/bin/cua-spaces' ] && grep -q 'Cua Spaces is macOS-only for now' '$work/out'"

inst -- --no-onboarding --prefix "$work/s3"
check "no tty, no --yes (Linux): CLI only, no hang" sh -c "[ $status = 0 ] && [ -e '$work/s3/bin/cua' ] && [ ! -e '$work/s3/bin/cua-spaces' ]"

inst CUA_INSTALL_OS=Darwin -- --dry-run --no-onboarding --prefix "$work/s4"
check "no tty (macOS dry run): plans the CLI and the app" sh -c "grep -q 'cua-cli-$version-darwin-x64.tar.gz' '$work/out' && grep -q 'cua-spaces-$version-darwin-universal.dmg' '$work/out'"
inst CUA_INSTALL_OS=Darwin CUA_INSTALL_DRIVER_URL="$driver_url" -- --dry-run --only cua-driver --prefix "$work/s4"
check "--only (macOS dry run): skips the app" sh -c "[ $status = 0 ] && ! grep -q 'darwin-universal.dmg' '$work/out'"

if command -v bash >/dev/null 2>&1; then
    inst CUA_INSTALL_DRIVER_URL="$driver_url" -- --no-onboarding --select cua-driver --prefix "$work/d1"
    check "--select cua-driver: runs the driver installer" sh -c "[ $status = 0 ] && grep -qx -- '--bin-dir $work/d1/bin --no-modify-path' '$home/driver.log'"
    check "--select cua-driver: the driver lands in --prefix/bin" exists "$work/d1/bin/cua-driver"
    check "--select cua-driver: registers the skill and MCP server" sh -c "grep -qx -- 'agents setup --cua-driver --agents all --yes --mcp-command $work/d1/bin/cua-driver' '$home/cua.log'"
    check "--select cua-driver: no host setup" sh -c "! grep -q 'host setup' '$home/cua.log'"

    if [ "$(id -u)" != 0 ]; then
        inst CUA_INSTALL_DRIVER_URL="$driver_url" -- --yes --no-onboarding --modify-path --select cua-driver
        check "--modify-path: driver may edit PATH; default bin dir" sh -c "grep -qx '' '$home/driver.log' && grep -q -- '--mcp-command $home/.local/bin/cua-driver' '$home/cua.log'"
    fi
else
    inst CUA_INSTALL_DRIVER_URL="$driver_url" -- --yes --only cua-driver --prefix "$work/d1"
    check "no bash: cua-driver fails early with a clear error" sh -c "[ $status != 0 ] && grep -q 'needs bash' '$work/out' && [ ! -e '$work/d1/bin/cua' ]"
fi

inst CUA_INSTALL_DRIVER_URL="http://example.invalid/install.sh" -- --yes --only cua-driver --prefix "$work/d2"
check "a plain-HTTP driver installer URL is refused before installing" sh -c "[ $status != 0 ] && grep -q 'non-HTTPS' '$work/out' && [ ! -e '$work/d2/bin/cua' ]"

inst CUA_INSTALL_NONINTERACTIVE=1 -- --select host --no-onboarding --prefix "$work/h1"
check "CUA_INSTALL_NONINTERACTIVE + --select host: runs 'cua host setup'" sh -c "[ $status = 0 ] && grep -qx 'host setup' '$home/cua.log'"

inst -- --yes --only host --prefix "$work/h2"
check "--only host: runs 'cua host setup', no app" sh -c "[ $status = 0 ] && grep -qx 'host setup' '$home/cua.log' && [ ! -e '$work/h2/bin/cua-spaces' ]"

inst CUA_INSTALL_DRIVER_URL="$driver_url" -- --dry-run --select cua-driver,host --prefix "$work/dr"
check "--dry-run prints the driver, agents and host steps" sh -c "grep -q 'dry-run. download $driver_url' '$work/out' && grep -q 'dry-run. bash cua-driver-install.sh --bin-dir' '$work/out' && grep -q 'dry-run. .* agents setup --cua-driver --agents all --yes' '$work/out' && grep -q 'dry-run. .* host setup' '$work/out'"
check "--dry-run runs nothing" sh -c "[ ! -e '$home/cua.log' ] && [ ! -e '$home/driver.log' ] && [ ! -e '$work/dr' ]"

inst -- --yes --select bogus
check "unknown item is an error listing the ids" sh -c "[ $status != 0 ] && grep -q \"unknown item 'bogus' (valid: cli, spaces, cua-driver, host)\" '$work/out'"
inst -- --yes --only host --select cua-driver
check "--only with --select is an error" sh -c "[ $status != 0 ] && grep -q 'mutually exclusive' '$work/out'"
inst -- --yes --cli-only --select spaces
check "--cli-only with --select spaces is an error" sh -c "[ $status != 0 ] && grep -q 'use --only spaces' '$work/out'"
inst -- --yes --app-only --only cua-driver
check "--app-only with cua-driver is an error" sh -c "[ $status != 0 ] && grep -q 'cua-driver and host need' '$work/out'"
inst -- --yes --cli-only --select cli --prefix "$work/c1"
check "--select cli is accepted and ignored" sh -c "[ $status = 0 ] && [ -e '$work/c1/bin/cua' ]"
inst -- --select
check "--select needs a value" sh -c "[ $status != 0 ] && grep -q 'needs a list' '$work/out'"

inst CUA_INSTALL_ARCH=aarch64 -- --yes --cli-only --prefix "$work/p2"
check "arch aarch64 maps to linux-arm64" sh -c "'$work/p2/bin/cua' --version | grep -q linux-arm64"
check "--cli-only skips the app" absent "$work/p2/bin/cua-spaces"

inst CUA_INSTALL_ARCH=mips -- --yes --prefix "$work/p3"
check "unsupported arch fails" sh -c "[ $status != 0 ] && grep -q 'unsupported CPU architecture' '$work/out'"

inst CUA_INSTALL_OS=MINGW64_NT-10.0 -- --yes
check "Windows shells point at install.ps1" sh -c "[ $status != 0 ] && grep -q install.ps1 '$work/out'"

inst CUA_INSTALL_OS=Darwin CUA_INSTALL_ARCH=x86_64 -- --yes --dry-run --cli-only --prefix "$work/p4"
check "darwin x86_64 maps to darwin-x64" has_out "cua-cli-$version-darwin-x64.tar.gz"
check "--dry-run installs nothing" absent "$work/p4"

inst -- --yes --app-only --mode host --prefix "$work/p5"
check "--app-only (Linux): exits 0 with a note" sh -c "[ $status = 0 ] && grep -q 'Cua Spaces is macOS-only for now' '$work/out'"
check "--app-only skips the CLI" absent "$work/p5/bin/cua"
check "--app-only (Linux): no app" absent "$work/p5/bin/cua-spaces"
check "--mode host (Linux): no install-mode file" absent "$home/.cua/spaces-install-mode"

inst -- --yes --no-onboarding --mode client --prefix "$work/p5b"
check "--mode client (Linux): installs the CLI, skips the app" sh -c "[ $status = 0 ] && [ -e '$work/p5b/bin/cua' ] && [ ! -e '$work/p5b/bin/cua-spaces' ] && [ ! -e '$home/.cua/spaces-install-mode' ]"

inst CUA_INSTALL_OS=Darwin -- --yes --dry-run --app-only --mode host --prefix "$work/p5c"
check "--mode host (macOS dry run): plans the app and the install-mode file" sh -c "[ $status = 0 ] && grep -q 'cua-spaces-$version-darwin-universal.dmg' '$work/out' && grep -q 'spaces-install-mode (host)' '$work/out'"

inst -- --yes --mode server
check "--mode rejects other values" sh -c "[ $status != 0 ] && grep -q 'host or client' '$work/out'"

inst -- --cli-only --app-only
check "--cli-only with --app-only is an error" sh -c "[ $status != 0 ] && grep -q 'mutually exclusive' '$work/out'"

inst -- --bogus
check "unknown option is an error" sh -c "[ $status != 0 ] && grep -q 'unknown option' '$work/out'"

inst -- --yes --cli-only --version "v$version" --prefix "$work/p6"
check "--version reads the versioned release manifest" has_out "cua-sdk-v$version/release-artifacts.json"
check "--version resolves relative artifact URLs" exists "$work/p6/bin/cua"

inst -- --yes --version 9.9.9 --prefix "$work/p7"
check "missing release version fails" sh -c "[ $status != 0 ] && grep -q 'download failed' '$work/out'"

inst -- --yes --version latest
check "--version validates its format" sh -c "[ $status != 0 ] && grep -q 'must look like' '$work/out'"

# Checksum mismatch: tamper with the served CLI archive.
bad="$release/bad"
mkdir -p "$bad"
cp "$tag_dir"/cua-* "$bad/"
sed 's/"sha256":"[0-9a-f]\{8\}/"sha256":"00000000/' "$tag_dir/release-artifacts.json" >"$bad/release-artifacts.json"
inst CUA_INSTALL_MANIFEST_URL="$base/bad/release-artifacts.json" -- --yes --prefix "$work/p8"
check "checksum mismatch aborts" sh -c "[ $status != 0 ] && grep -q 'checksum mismatch' '$work/out'"
check "checksum mismatch installs nothing" absent "$work/p8/bin/cua"

inst CUA_INSTALL_MANIFEST_URL="$base/cua-sdk-v$version/cua-cli-$version-linux-x64.tar.gz" -- --yes
check "a non-manifest file is rejected" sh -c "[ $status != 0 ] && grep -q 'unsupported release manifest' '$work/out'"

# Downloads stay on HTTPS: a manifest pointing at plain HTTP (off loopback)
# or file:// is refused before anything is fetched.
insecure="$release/insecure"
mkdir -p "$insecure"
sed 's#"url":"[^"]*/\(cua-[^"]*\)"#"url":"http://example.invalid/\1"#' \
    "$latest_dir/release-artifacts.json" >"$insecure/release-artifacts.json"
inst CUA_INSTALL_MANIFEST_URL="$base/insecure/release-artifacts.json" -- --yes --cli-only --prefix "$work/p16"
check "plain-HTTP artifact URL is refused" sh -c "[ $status != 0 ] && grep -q 'non-HTTPS' '$work/out' && [ ! -e '$work/p16/bin/cua' ]"
sed 's#"url":"[^"]*/\(cua-[^"]*\)"#"url":"file:///etc/\1"#' \
    "$latest_dir/release-artifacts.json" >"$insecure/file.json"
inst CUA_INSTALL_MANIFEST_URL="$base/insecure/file.json" -- --yes --cli-only --prefix "$work/p17"
check "file:// artifact URL from a remote manifest is refused" sh -c "[ $status != 0 ] && grep -q 'remote manifest' '$work/out'"
inst CUA_INSTALL_MANIFEST_URL="http://example.invalid/release-artifacts.json" -- --yes --cli-only --prefix "$work/p18"
check "plain-HTTP manifest URL is refused" sh -c "[ $status != 0 ] && grep -q 'non-HTTPS' '$work/out'"
inst CUA_INSTALL_MANIFEST_URL="http://127.0.0.1:1@example.invalid/release-artifacts.json" -- --yes --cli-only --prefix "$work/p19"
check "loopback-lookalike URL with userinfo is refused" sh -c "[ $status != 0 ] && grep -q 'malformed' '$work/out'"

# Signatures: --require-signature without minisign/cosign entries fails.
inst -- --yes --cli-only --require-signature --prefix "$work/p9"
check "--require-signature fails without a verifiable signature" sh -c "[ $status != 0 ] && grep -q 'no verifiable signature' '$work/out' && [ ! -e '$work/p9/bin/cua' ]"

# A failing minisign aborts; a passing one satisfies --require-signature.
sig="$release/sig"
mkdir -p "$sig" "$work/fakebin-bad" "$work/fakebin-good"
cp "$tag_dir"/cua-cli-* "$sig/"
for f in "$sig"/cua-cli-*.tar.gz; do printf 'sig\n' >"$f.minisig"; done
python3 "$manifest_tool" --component cli --version "$version" --dir "$sig" --out "$sig/release-artifacts.json"
printf '#!/bin/sh\nexit 1\n' >"$work/fakebin-bad/minisign"
printf '#!/bin/sh\nexit 0\n' >"$work/fakebin-good/minisign"
chmod +x "$work/fakebin-bad/minisign" "$work/fakebin-good/minisign"
inst PATH="$work/fakebin-bad:$PATH" CUA_INSTALL_MINISIGN_PUBKEY=RWTEST CUA_INSTALL_MANIFEST_URL="$base/sig/release-artifacts.json" -- --yes --cli-only --prefix "$work/p10"
check "bad minisign signature aborts" sh -c "[ $status != 0 ] && grep -q 'signature check failed' '$work/out' && [ ! -e '$work/p10/bin/cua' ]"
inst PATH="$work/fakebin-good:$PATH" CUA_INSTALL_MINISIGN_PUBKEY=RWTEST CUA_INSTALL_MANIFEST_URL="$base/sig/release-artifacts.json" -- --yes --cli-only --require-signature --prefix "$work/p11"
check "good minisign signature satisfies --require-signature" exists "$work/p11/bin/cua"
if command -v bash >/dev/null 2>&1; then
    inst PATH="$work/fakebin-good:$PATH" CUA_INSTALL_MINISIGN_PUBKEY=RWTEST CUA_INSTALL_MANIFEST_URL="$base/sig/release-artifacts.json" \
        CUA_INSTALL_DRIVER_URL="$driver_url" -- --yes --only cua-driver --require-signature --prefix "$work/p11d"
    check "--require-signature is forwarded to the cua-driver installer" sh -c "[ $status = 0 ] && grep -qx -- '--bin-dir $work/p11d/bin --no-modify-path --require-signature' '$home/driver.log'"
fi

# cosign: the bundle must name the exact release workflow at the CLI's tag.
csig="$release/csig"
mkdir -p "$csig" "$work/fakebin-cosign"
cp "$tag_dir"/cua-cli-* "$csig/"
for f in "$csig"/cua-cli-*.tar.gz; do printf '{}\n' >"$f.sigstore.json"; done
python3 "$manifest_tool" --component cli --version "$version" --dir "$csig" --out "$csig/release-artifacts.json"
want_identity="https://github.com/trycua/cua/.github/workflows/cd-cua-sdk.yml@refs/tags/cua-sdk-v$version"
cat >"$work/fakebin-cosign/cosign" <<COSIGN
#!/bin/sh
printf '%s\n' "\$*" >>"$work/cosign.log"
prev=""
for a in "\$@"; do
    if [ "\$prev" = --certificate-identity ] && [ "\$a" = "$want_identity" ]; then exit 0; fi
    case "\$a" in --certificate-identity-regexp*) exit 1 ;; esac
    prev="\$a"
done
exit 1
COSIGN
chmod +x "$work/fakebin-cosign/cosign"
inst PATH="$work/fakebin-cosign:$PATH" CUA_INSTALL_MANIFEST_URL="$base/csig/release-artifacts.json" -- --yes --cli-only --require-signature --prefix "$work/p16"
check "cosign verifies the exact release workflow and tag" sh -c "exists() { [ -e \"\$1\" ]; }; [ $status = 0 ] && [ -e '$work/p16/bin/cua' ] && grep -q -- '--certificate-identity $want_identity' '$work/cosign.log'"
printf '#!/bin/sh\nexit 1\n' >"$work/fakebin-cosign/cosign"
inst PATH="$work/fakebin-cosign:$PATH" CUA_INSTALL_MANIFEST_URL="$base/csig/release-artifacts.json" -- --yes --cli-only --prefix "$work/p17"
check "a cosign bundle from another identity aborts" sh -c "[ $status != 0 ] && grep -q 'cosign signature check failed' '$work/out' && [ ! -e '$work/p17/bin/cua' ]"

# --modify-path appends once to the shell profile.
inst -- --yes --cli-only --modify-path --prefix "$work/p12"
check "--modify-path writes the profile" sh -c "grep -q '$work/p12/bin' '$home/.profile'"
first_home="$home"
env -i PATH="$PATH" HOME="$first_home" SHELL=/bin/sh CUA_INSTALL_BASE_URL="$base" CUA_INSTALL_OS=linux \
    CUA_INSTALL_ARCH=x86_64 "$sh_bin" "$installer" --yes --cli-only --modify-path --prefix "$work/p12" </dev/null >"$work/out" 2>&1
check "--modify-path is idempotent" [ "$(grep -c "$work/p12/bin" "$first_home/.profile")" = 1 ]

inst SHELL=/bin/zsh -- --yes --cli-only --modify-path --prefix "$work/p13"
check "--modify-path picks .zshrc for zsh" sh -c "grep -q '$work/p13/bin' '$home/.zshrc'"

# No --prefix, non-root: ~/.local/bin under the fake HOME.
if [ "$(id -u)" != 0 ]; then
    inst -- --yes --cli-only
    check "default bin dir is ~/.local/bin" exists "$home/.local/bin/cua"
fi

# Root with apt-get and no --prefix: the .deb path (dry run only).
if [ "$(id -u)" = 0 ] && command -v apt-get >/dev/null 2>&1; then
    inst -- --yes --app-only --dry-run
    check "root + apt: the app is skipped, no .deb" sh -c "[ $status = 0 ] && ! grep -q 'apt-get install' '$work/out' && grep -q 'Cua Spaces is macOS-only for now' '$work/out'"
fi

inst -- --help
check "--help prints usage" sh -c "[ $status = 0 ] && grep -q -- '--cli-only' '$work/out'"

# Interactive onboarding runs `cua auth login` when a tty is available.
if script --version 2>/dev/null | grep -q util-linux; then
    home="$work/home.tty"
    mkdir -p "$home"
    env -i PATH="$PATH" HOME="$home" SHELL=/bin/sh CUA_INSTALL_BASE_URL="$base" CUA_INSTALL_OS=linux \
        CUA_INSTALL_ARCH=x86_64 CUA_FAKE_LOG="$home/cua.log" \
        script -qec "$sh_bin '$installer' --yes --cli-only --prefix '$work/p14'" /dev/null >"$work/out" 2>&1 </dev/null
    check "interactive: runs 'cua auth login'" sh -c "grep -qx 'auth login' '$home/cua.log'"
    env -i PATH="$PATH" HOME="$home" SHELL=/bin/sh CUA_INSTALL_BASE_URL="$base" CUA_INSTALL_OS=linux \
        CUA_INSTALL_ARCH=x86_64 CUA_FAKE_LOG="$home/cua2.log" \
        script -qec "$sh_bin '$installer' --yes --cli-only --no-onboarding --prefix '$work/p14'" /dev/null >"$work/out" 2>&1 </dev/null
    check "interactive + --no-onboarding: no login" absent "$home/cua2.log"

    # The checklist, driven through a pty. Keys arrive after the prompt shows.
    # tty_inst KEYS ARGS...: run install.sh in a pty with a fresh HOME.
    tty_inst() {
        keys="$1"
        shift
        home="$work/home.tty.$RANDOM_SEQ"
        RANDOM_SEQ=$((RANDOM_SEQ + 1))
        mkdir -p "$home"
        (
            sleep 2
            printf '%b' "$keys"
            sleep 3
        ) | env -i PATH="$PATH" HOME="$home" SHELL=/bin/sh TERM="${tty_term:-xterm}" CUA_HOME="$home/.cua" \
            CUA_INSTALL_BASE_URL="$base" CUA_INSTALL_OS=linux CUA_INSTALL_ARCH=x86_64 CUA_FAKE_LOG="$home/cua.log" \
            CUA_INSTALL_DRIVER_URL="$driver_url" \
            timeout 60 script -qec "$sh_bin '$installer' --no-onboarding $*" /dev/null >"$work/out" 2>&1
        status=$?
    }
    logged() { grep -q -- "$1" "$home/cua.log" 2>/dev/null; }
    if command -v bash >/dev/null 2>&1 && command -v timeout >/dev/null 2>&1; then
        tty_inst '\r' --prefix "$work/t1"
        check "checklist: shows the rows, Linux hides Spaces" sh -c "grep -q 'Choose what to install' '$work/out' && grep -q 'cua CLI (required)' '$work/out' && grep -q 'cua-driver MCP and skill for your agents' '$work/out' && ! grep -q 'Cua Spaces app' '$work/out'"
        check "checklist: Enter accepts the defaults (CLI only)" sh -c "[ $status = 0 ] && [ -e '$work/t1/bin/cua' ] && [ ! -e '$work/t1/bin/cua-driver' ] && [ ! -e '$work/t1/bin/cua-spaces' ]"
        check "checklist: no per-item prompt afterwards" sh -c "! grep -q 'Install the cua CLI to' '$work/out'"
        tty_inst '2\r' --prefix "$work/t2"
        check "checklist: number key toggles cua-driver on" sh -c "[ $status = 0 ] && [ -e '$work/t2/bin/cua-driver' ] && grep -q 'agents setup --cua-driver' '$home/cua.log'"
        tty_inst '\033[B\033[B \r' --prefix "$work/t3"
        check "checklist: arrows + space select host" sh -c "[ $status = 0 ] && grep -qx 'host setup' '$home/cua.log' && [ ! -e '$work/t3/bin/cua-driver' ]"
        tty_inst '1 \033[A\r' --prefix "$work/t4"
        check "checklist: the CLI row is locked" sh -c "[ $status = 0 ] && [ -e '$work/t4/bin/cua' ] && [ ! -e '$work/t4/bin/cua-driver' ]"
        tty_inst '\r' --select cua-driver --prefix "$work/t5"
        check "checklist: --select cua-driver is preselected" sh -c "[ $status = 0 ] && [ -e '$work/t5/bin/cua-driver' ] && grep -q '\\[x\\] cua-driver' '$work/out'"
        tty_inst '2\r' --select cua-driver --prefix "$work/t6"
        check "checklist: a preselected item can be turned off" sh -c "[ $status = 0 ] && [ -e '$work/t6/bin/cua' ] && [ ! -e '$work/t6/bin/cua-driver' ]"
        tty_inst '\r' --select spaces --prefix "$work/t7"
        check "checklist: --select spaces on Linux: no Spaces row, not installed" sh -c "[ $status = 0 ] && ! grep -q 'Cua Spaces app' '$work/out' && [ ! -e '$work/t7/bin/cua-spaces' ] && [ -e '$work/t7/bin/cua' ]"
        tty_inst '' --prefix "$work/t8"
        check "checklist: no input (EOF) accepts defaults, no hang" sh -c "[ $status = 0 ] && [ -e '$work/t8/bin/cua' ]"
        tty_inst '\r' --yes --select host --prefix "$work/t9"
        check "checklist: --yes skips it" sh -c "[ $status = 0 ] && ! grep -q 'Choose what to install' '$work/out' && grep -qx 'host setup' '$home/cua.log'"
        tty_inst '\r' --only cua-driver --prefix "$work/t10"
        check "checklist: --only skips it" sh -c "[ $status = 0 ] && ! grep -q 'Choose what to install' '$work/out' && [ -e '$work/t10/bin/cua-driver' ]"
        tty_term=dumb
        tty_inst '3\n\n' --prefix "$work/t11"
        tty_term=""
        check "checklist (TERM=dumb): numbered prompt toggles host" sh -c "[ $status = 0 ] && grep -q 'Toggle numbers' '$work/out' && grep -qx 'host setup' '$home/cua.log'"
        [ -n "${CUA_INSTALL_TRANSCRIPT:-}" ] && tty_inst '\033[B \033[B \r' --prefix "$work/t12" && cp "$work/out" "$CUA_INSTALL_TRANSCRIPT"
    else
        printf 'skip interactive checklist (needs bash and timeout)\n'
    fi
else
    printf 'skip interactive onboarding (needs util-linux script)\n'
fi

if [ "$(uname -s)" = Darwin ]; then
    # The app needs macOS 26; the runner may be older, so a fake sw_vers
    # reports the version (the dmg mount and copy are still real).
    mkdir -p "$work/fakebin-macos26" "$work/fakebin-macos14"
    printf '#!/bin/sh\necho 26.0\n' >"$work/fakebin-macos26/sw_vers"
    printf '#!/bin/sh\necho 14.8\n' >"$work/fakebin-macos14/sw_vers"
    chmod +x "$work/fakebin-macos26/sw_vers" "$work/fakebin-macos14/sw_vers"
    inst PATH="$work/fakebin-macos14:$PATH" CUA_INSTALL_OS= CUA_INSTALL_ARCH= -- --yes --app-only --prefix "$work/p15a"
    check "macOS 14: skips the app (needs macOS 26)" sh -c "[ $status = 0 ] && grep -q 'needs macOS 26 or later' '$work/out' && [ ! -e '$work/p15a/Applications/Cua Spaces.app' ]"
    inst PATH="$work/fakebin-macos26:$PATH" CUA_INSTALL_OS= CUA_INSTALL_ARCH= -- --yes --app-only --prefix "$work/p15"
    check "macOS: mounts the dmg and copies the app into --prefix/Applications" exists "$work/p15/Applications/Cua Spaces.app/Contents/MacOS/cua-spaces"
    check "macOS: detaches the dmg" sh -c "! hdiutil info | grep -q '$work'"
    inst PATH="$work/fakebin-macos26:$PATH" CUA_INSTALL_OS= CUA_INSTALL_ARCH= -- --no-onboarding --prefix "$work/p15b"
    check "macOS no tty: installs the CLI and the app by default" sh -c "[ -e '$work/p15b/bin/cua' ] && [ -e '$work/p15b/Applications/Cua Spaces.app' ]"
else
    printf 'skip macOS dmg install (not macOS)\n'
fi

printf '\n%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" = 0 ]
