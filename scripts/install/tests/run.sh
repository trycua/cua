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
if [ "\${1:-} \${2:-} \${3:-}" = "host setup --help" ]; then
    echo "Usage: cua host setup [OPTIONS]"
    [ -n "\${CUA_FAKE_NO_PAIR:-}" ] || echo "      --pair <CODE>  add this machine with a code"
    exit 0
fi
echo "\$*" >>"\${CUA_FAKE_LOG:-/dev/null}"
if [ "\$*" = "runtime doctor" ]; then
    echo 'runtime doctor diagnostic'
    exit "\${CUA_FAKE_DOCTOR_STATUS:-0}"
fi
if [ "\$*" = "runtime setup qemu" ]; then
    [ -z "\${CUA_FAKE_QEMU_DIR:-}" ] || for t in qemu-img qemu-system-\$(uname -m); do
        printf '#!/bin/sh\n' >"\$CUA_FAKE_QEMU_DIR/\$t" && chmod +x "\$CUA_FAKE_QEMU_DIR/\$t"
    done
    exit "\${CUA_FAKE_RUNTIME_STATUS:-0}"
fi
EOF
    chmod +x "$1/cua"
}

for platform in linux-x64 linux-arm64 darwin-arm64 darwin-x64; do
    make_cua "$work/stage/$platform" "$platform"
    extra=""
    # Apple silicon: the microVM monitor and guest init, and the static
    # cua-spacesd injected into OCI images, travel with `cua`.
    if [ "$platform" = darwin-arm64 ]; then
        for f in cua-vz cua-microvm-init cua-spacesd-linux-aarch64; do
            printf '#!/bin/sh\necho %s\n' "$f" >"$work/stage/$platform/$f"
        done
        extra="cua-vz cua-microvm-init cua-spacesd-linux-aarch64"
    fi
    # shellcheck disable=SC2086
    tar -czf "$tag_dir/cua-cli-$version-$platform.tar.gz" -C "$work/stage/$platform" cua $extra
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

mkdir -p "$work/qemu" "$work/host-bin"
for f in $(printf '%s' "$PATH" | tr ':' '\n' | sed 's#$#/*#'); do
    case "${f##*/}" in qemu* | docker) continue ;; esac
    [ -x "$f" ] && [ ! -e "$work/host-bin/${f##*/}" ] && ln -s "$f" "$work/host-bin/${f##*/}"
done
host_path="$work/host-bin"
for t in qemu-img "qemu-system-$(uname -m)"; do printf '#!/bin/sh\n' >"$work/qemu/$t"; done
chmod +x "$work/qemu"/*
PATH="$work/qemu:$PATH"

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
    # Install telemetry may only reach a loopback endpoint here (none is set).
    env -i PATH="$PATH" HOME="$home" SHELL=/bin/sh CUA_HOME="$home/.cua" CUA_TELEMETRY_FORBID_NETWORK=1 \
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

inst -- --yes --no-onboarding --verbose --prefix "$work/p1"
check "linux x64: exits 0" [ "$status" = 0 ]
check "linux x64: installs cua under --prefix/bin" exists "$work/p1/bin/cua"
check "linux x64: installed cua runs and is the x64 build" sh -c "'$work/p1/bin/cua' --version | grep -q linux-x64"
check "linux default: installs the Electron AppImage" sh -c "[ -x '$work/p1/bin/cua-spaces' ] && '$work/p1/bin/cua-spaces' | grep -q 'cua-spaces x64'"
check "linux default: writes the desktop entry" exists "$work/p1/share/applications/cua-spaces.desktop"
check "linux default: no cua-driver" absent "$home/driver.log"
check "linux x64: uses the rolling latest manifest" has_out "cua-install-latest/release-artifacts.json"
check "non-interactive: prints next steps, no login" sh -c "grep -q 'cua auth login' '$work/out' && ! grep -q 'auth login' '$home/cua.log'"
check "prefix bin not on PATH: prints a hint" has_out "is not on your PATH"
check "no --modify-path: profile untouched" absent "$home/.profile"
check "--verbose: shows the platform and manifest" has_out "cua-install: platform linux-x64, manifest "
check "--verbose: shows each download" has_out "cua-install: downloading cua-cli-$version-linux-x64.tar.gz"

check "VM runtime present: no runtime setup" sh -c "grep -q 'VM runtime ok' '$work/out' && ! grep -q 'runtime setup' '$home/cua.log' 2>/dev/null"

# The default output: one plain line per step, then what was installed and
# what to do next. The cua-install: detail lines are --verbose only.
inst -- --yes --no-onboarding --prefix "$work/q1"
check "quiet default: no cua-install: lines" sh -c "[ $status = 0 ] && ! grep -q '^cua-install:' '$work/out'"
check "quiet default: one 'Installing' line per step" sh -c "[ \"\$(grep -c '^Installing ' '$work/out')\" = 2 ] && grep -q '^Installing the command-line tools to ' '$work/out' && grep -q '^Installing the Cua Spaces app to ' '$work/out'"
check "quiet default: closes with what was installed" has_out "^Installed the command-line tools and the Cua Spaces app (downloads checked)\\.\$"
check "quiet default: says what to do next" sh -c "grep -q '^Next: run .*cua-spaces. It guides you through signing in and connecting your AI agents.' '$work/out' && grep -q 'Prefer the Terminal? Run \"cua auth login\" to sign in, then \"cua agents setup\"' '$work/out'"
check "quiet default: no signature, checksum or manifest jargon" sh -c "! grep -qi 'cosign\|minisign\|sha256\|checksum\|manifest' '$work/out'"
check "quiet default: the PATH hint comes last" sh -c "tail -4 '$work/out' | grep -q 'is not on your PATH'"
inst CUA_INSTALL_VERBOSE=1 -- --yes --no-onboarding --cli-only --prefix "$work/q2"
check "CUA_INSTALL_VERBOSE=1 is --verbose" has_out "^cua-install: platform "
inst -- --yes --no-onboarding --cli-only --prefix "$work/q3"
check "--cli-only: summary points at sign-in, agents and hosting" sh -c "grep -q '^Installed the command-line tools (downloads checked)' '$work/out' && grep -q '^Next: run \"cua auth login\" to sign in, then \"cua agents setup\" to connect your AI agents.' '$work/out' && grep -q 'No web browser on this computer? Run \"cua auth login --remote\"' '$work/out' && grep -q 'To let your other devices use this computer, run \"cua host setup\".' '$work/out'"
inst -- --yes --no-onboarding --app-only --prefix "$work/q4"
check "--app-only: summary names the app, not the CLI" sh -c "grep -q '^Installed the Cua Spaces app (downloads checked)' '$work/out' && ! grep -q 'cua auth login' '$work/out'"
inst -- --no-onboarding --cli-only --prefix "$work/q5"
check "no terminal and no --yes: says it uses the defaults, once" sh -c "[ $status = 0 ] && [ \"\$(grep -c 'ask you questions here' '$work/out')\" = 1 ]"
inst -- --yes --no-onboarding --cli-only --prefix "$work/q6"
check "--yes: no note about not asking questions" sh -c "! grep -q 'ask you questions' '$work/out'"
inst CUA_INSTALL_NONINTERACTIVE=1 -- --no-onboarding --cli-only --prefix "$work/q7"
check "CUA_INSTALL_NONINTERACTIVE=1: no note about not asking questions" sh -c "! grep -q 'ask you questions' '$work/out'"

mkdir -p "$work/qemu-r1"
inst PATH="$work/qemu-r1:$host_path" CUA_FAKE_QEMU_DIR="$work/qemu-r1" -- --yes --no-onboarding --prefix "$work/r1"
check "VM runtime missing, setup succeeds: installs the app" sh -c "[ $status = 0 ] && grep -qx 'runtime setup qemu' '$home/cua.log' && [ -x '$work/r1/bin/cua-spaces' ]"
inst PATH="$host_path" CUA_FAKE_RUNTIME_STATUS=1 -- --yes --no-onboarding --prefix "$work/r2"
check "VM runtime setup fails: finishes with fix and doctor even when doctor succeeds" sh -c "[ $status = 1 ] && grep -q 'needs QEMU or Docker; install QEMU with: sudo .* install -y qemu' '$work/out' && grep -q 'runtime doctor diagnostic' '$work/out' && grep -q '^Next:' '$work/out' && [ -x '$work/r2/bin/cua-spaces' ]"
inst PATH="$host_path" -- --yes --no-onboarding --prefix "$work/r3"
check "VM runtime setup leaves QEMU missing: exits with the fix" sh -c "[ $status != 0 ] && grep -q 'needs QEMU or Docker' '$work/out' && [ -x '$work/r3/bin/cua-spaces' ]"
inst PATH="$host_path" -- --dry-run --no-onboarding --prefix "$work/r4"
check "VM runtime missing, --dry-run: prints the setup only" sh -c "[ $status = 0 ] && grep -q 'dry-run. $work/r4/bin/cua runtime setup qemu' '$work/out' && [ ! -e '$work/r4' ] && [ ! -e '$home/cua.log' ]"

inst CUA_FAKE_DOCTOR_STATUS=1 -- --yes --no-onboarding --prefix "$work/r5"
check "doctor-only failure finishes installation" sh -c "[ $status = 1 ] && grep -q 'runtime doctor diagnostic' '$work/out' && grep -q '^Next:' '$work/out' && [ -x '$work/r5/bin/cua-spaces' ]"

# ------------------------------------------------------------ selection

# Cua Spaces ships for macOS and Linux; the manifest also lists Windows.
inst -- --yes --no-onboarding --verbose --select spaces --prefix "$work/s1"
check "--select spaces (Linux): installs the AppImage" sh -c "[ $status = 0 ] && [ -x '$work/s1/bin/cua-spaces' ] && grep -q 'cua-spaces-$version-linux-x64.AppImage' '$work/out'"
check "--select spaces (Linux): desktop entry" exists "$work/s1/share/applications/cua-spaces.desktop"
check "--select spaces (Linux): still installs the CLI" exists "$work/s1/bin/cua"

inst -- --no-onboarding --only spaces --prefix "$work/s2"
check "--only spaces (Linux): installs the CLI and the app" sh -c "[ $status = 0 ] && [ -x '$work/s2/bin/cua-spaces' ] && [ -e '$work/s2/bin/cua' ]"

inst CUA_INSTALL_ARCH=aarch64 -- --yes --no-onboarding --verbose --select spaces --prefix "$work/s2b"
check "--select spaces (Linux arm64): installs the arm64 AppImage" sh -c "[ $status = 0 ] && grep -q 'cua-spaces-$version-linux-arm64.AppImage' '$work/out' && '$work/s2b/bin/cua-spaces' | grep -q 'cua-spaces arm64'"

inst -- --no-onboarding --prefix "$work/s3"
check "no tty, no --yes (Linux): installs with the defaults, no hang" sh -c "[ $status = 0 ] && [ -e '$work/s3/bin/cua' ] && [ -x '$work/s3/bin/cua-spaces' ]"

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
    check "--select cua-driver: adds the skill by default (no --no-skills)" sh -c "! grep -q -- '--no-skills' '$home/cua.log'"

    inst CUA_INSTALL_DRIVER_URL="$driver_url" -- --no-onboarding --select cua-driver --no-skills --prefix "$work/d1s"
    check "--no-skills: still registers the MCP server, without the skill" sh -c "[ $status = 0 ] && grep -qx -- 'agents setup --cua-driver --agents all --yes --no-skills --mcp-command $work/d1s/bin/cua-driver' '$home/cua.log'"

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

inst -- --pair k7qm-4xpb-2rhd --prefix "$work/pr1"
check "--pair: runs 'cua host setup --pair', no sign-in, no checklist" sh -c "[ $status = 0 ] && grep -qx 'host setup --pair k7qm-4xpb-2rhd' '$home/cua.log' && ! grep -q 'auth login' '$home/cua.log' && grep -q 'This machine is added' '$work/out'"

inst CUA_FAKE_NO_PAIR=1 -- --pair K7QM-4XPB-2RHD --prefix "$work/pr0"
check "--pair: a CLI without 'host setup --pair' fails early with a clear message" sh -c "[ $status != 0 ] && grep -q \"no 'cua host setup --pair'\" '$work/out' && ! grep -q 'host setup --pair' '$home/cua.log' 2>/dev/null"

inst -- --pair K7QM-4XPB-2RHD --relay https://relay.example.com --cli-only --prefix "$work/pr2"
check "--pair --relay --cli-only: passes the relay on" sh -c "[ $status = 0 ] && grep -qx 'host setup --pair K7QM-4XPB-2RHD --relay https://relay.example.com' '$home/cua.log'"

inst -- --pair 'K7QM;rm -rf /' --prefix "$work/pr3"
check "--pair: refuses a code with shell characters before installing" sh -c "[ $status != 0 ] && grep -q 'not a pairing code' '$work/out' && [ ! -e '$work/pr3/bin/cua' ]"

inst -- --pair K7QM-4XPB-2RHD --relay http://evil.example --prefix "$work/pr4"
check "--pair: refuses a plain-http relay" sh -c "[ $status != 0 ] && grep -q 'must be an https' '$work/out'"

inst -- --pair K7QM-4XPB-2RHD --relay http://localhost.evil.example --prefix "$work/pr5"
check "--pair: refuses a plain-http relay that only starts like localhost" sh -c "[ $status != 0 ] && grep -q 'must be an https' '$work/out'"

inst -- --pair K7QM-4XPB-2RHD --relay http://127.0.0.1:8080 --prefix "$work/pr6"
check "--pair: allows a local plain-http relay" sh -c "[ $status = 0 ] && grep -qx 'host setup --pair K7QM-4XPB-2RHD --relay http://127.0.0.1:8080' '$home/cua.log'"
check "--pair: no sign-in next steps" sh -c "! grep -q 'cua auth login' '$work/out'"

inst CUA_INSTALL_DRIVER_URL="$driver_url" -- --dry-run --select cua-driver,host --prefix "$work/dr"
check "--dry-run prints the driver, agents and host steps" sh -c "grep -q 'dry-run. download $driver_url' '$work/out' && grep -q 'dry-run. bash cua-driver-install.sh --bin-dir' '$work/out' && grep -q 'dry-run. .* agents setup --cua-driver --agents all --yes' '$work/out' && grep -q 'dry-run. .* host setup' '$work/out'"
inst CUA_INSTALL_DRIVER_URL="$driver_url" -- --dry-run --select cua-driver --no-skills --prefix "$work/drs"
check "--dry-run --no-skills: the planned agent setup leaves the skill out" sh -c "grep -q 'dry-run. .* agents setup --cua-driver --agents all --yes --no-skills' '$work/out'"
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

# Never a downgrade (UL2): a newer cua already installed stays; an older one
# is upgraded; --version asks for a rollback on purpose.
fake_cua() { # dir version
    mkdir -p "$1/bin"
    # shellcheck disable=SC2016 # the single quotes are deliberate: the body expands when the stub runs
    printf '#!/bin/sh\nif [ "${1:-}" = --version ]; then echo "cua %s"; exit 0; fi\n' "$2" >"$1/bin/cua"
    chmod +x "$1/bin/cua"
}
fake_cua "$work/nd1" 9.9.9
inst -- --yes --cli-only --prefix "$work/nd1"
check "a newer installed cua is kept" sh -c "[ $status = 0 ] && '$work/nd1/bin/cua' --version | grep -qx 'cua 9.9.9' && grep -q 'kept .*cua 9.9.9 is newer' '$work/out'"
fake_cua "$work/nd2" 0.1.0
inst -- --yes --cli-only --prefix "$work/nd2"
check "an older installed cua is upgraded" sh -c "[ $status = 0 ] && '$work/nd2/bin/cua' --version | grep -q 'cua $version'"
fake_cua "$work/nd3" 9.9.9
inst -- --yes --cli-only --version "$version" --prefix "$work/nd3"
check "--version rolls back a newer cua on purpose" sh -c "[ $status = 0 ] && '$work/nd3/bin/cua' --version | grep -q 'cua $version'"
fake_cua "$work/nd4" 1.2.3-dev
inst -- --yes --cli-only --prefix "$work/nd4"
check "a dev build installed counts as older" sh -c "[ $status = 0 ] && '$work/nd4/bin/cua' --version | grep -q 'cua $version ('"

inst CUA_INSTALL_ARCH=mips -- --yes --prefix "$work/p3"
check "unsupported arch fails" sh -c "[ $status != 0 ] && grep -q 'unsupported CPU architecture' '$work/out'"

inst CUA_INSTALL_OS=MINGW64_NT-10.0 -- --yes
check "Windows shells point at install.ps1" sh -c "[ $status != 0 ] && grep -q install.ps1 '$work/out'"

inst CUA_INSTALL_OS=Darwin CUA_INSTALL_ARCH=arm64 -- --yes --no-onboarding --cli-only --prefix "$work/mvz"
check "darwin arm64: cua-vz, the microVM guest init and the static cua-spacesd land next to cua" sh -c "[ $status = 0 ] && [ -x '$work/mvz/bin/cua' ] && [ -x '$work/mvz/bin/cua-vz' ] && [ -x '$work/mvz/bin/cua-microvm-init' ] && [ -x '$work/mvz/bin/cua-spacesd-linux-aarch64' ]"
inst -- --yes --no-onboarding --cli-only --prefix "$work/nvz"
check "linux: no microVM monitor in the archive, none installed" sh -c "[ $status = 0 ] && [ -e '$work/nvz/bin/cua' ] && [ ! -e '$work/nvz/bin/cua-vz' ]"

inst CUA_INSTALL_OS=Darwin CUA_INSTALL_ARCH=x86_64 -- --yes --dry-run --cli-only --prefix "$work/p4"
check "darwin x86_64 maps to darwin-x64" has_out "cua-cli-$version-darwin-x64.tar.gz"
check "--dry-run installs nothing" absent "$work/p4"

inst -- --yes --app-only --mode host --prefix "$work/p5"
check "--app-only (Linux): exits 0" [ "$status" = 0 ]
check "--app-only skips the CLI" absent "$work/p5/bin/cua"
check "--app-only (Linux): installs the app" sh -c "[ -x '$work/p5/bin/cua-spaces' ]"
check "--mode host (Linux): writes the install-mode file" sh -c "grep -q host '$home/.cua/spaces-install-mode'"

inst -- --yes --no-onboarding --mode client --prefix "$work/p5b"
check "--mode client (Linux): installs the CLI and the app" sh -c "[ $status = 0 ] && [ -e '$work/p5b/bin/cua' ] && [ -x '$work/p5b/bin/cua-spaces' ] && grep -q client '$home/.cua/spaces-install-mode'"

inst CUA_INSTALL_OS=Darwin -- --yes --dry-run --app-only --mode host --prefix "$work/p5c"
check "--mode host (macOS dry run): plans the app and the install-mode file" sh -c "[ $status = 0 ] && grep -q 'cua-spaces-$version-darwin-universal.dmg' '$work/out' && grep -q 'spaces-install-mode (host)' '$work/out'"

inst -- --yes --mode server
check "--mode rejects other values" sh -c "[ $status != 0 ] && grep -q 'host or client' '$work/out'"

inst -- --cli-only --app-only
check "--cli-only with --app-only is an error" sh -c "[ $status != 0 ] && grep -q 'mutually exclusive' '$work/out'"

inst -- --bogus
check "unknown option is an error" sh -c "[ $status != 0 ] && grep -q 'unknown option' '$work/out'"

inst -- --yes --cli-only --verbose --version "v$version" --prefix "$work/p6"
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
    check "root + apt: the app is the .deb" sh -c "[ $status = 0 ] && grep -q 'apt-get install' '$work/out' && grep -q 'cua-spaces-$version-linux-x64.deb' '$work/out'"
fi

# PATH: not on PATH prints one exact line for the detected shell; never edits
# the profile without consent; --no-modify-path stays silent about editing.
inst SHELL=/bin/zsh -- --yes --cli-only --prefix "$work/pp1"
check "PATH hint: exact one-liner for the user's shell (.zshrc)" sh -c "grep -qF \"echo 'export PATH=\\\"$work/pp1/bin:\\\$PATH\\\"' >> $home/.zshrc\" '$work/out' && [ ! -e '$home/.zshrc' ]"
inst -- --yes --cli-only --no-modify-path --prefix "$work/pp2"
check "--no-modify-path: profile untouched, hint printed" sh -c "[ $status = 0 ] && grep -q 'is not on your PATH' '$work/out' && [ ! -e '$home/.profile' ]"
inst -- --yes --cli-only --modify-path --no-modify-path --prefix "$work/pp3"
check "--no-modify-path wins over --modify-path" absent "$home/.profile"
inst -- --pair K7QM-4XPB-2RHD --prefix "$work/pp4"
check "--pair runs the CLI by absolute path even when it is not on PATH" sh -c "[ $status = 0 ] && grep -qx 'host setup --pair K7QM-4XPB-2RHD' '$home/cua.log' && grep -q 'This machine is added' '$work/out'"

# --pair on a computer that already has Cua: reuse the cua and the app that are
# there instead of downloading them again.
seed="$work/seed/bin"
make_cua "$seed" linux-x64
sed -i.bak "s/cua $version/cua 9.9.9/" "$seed/cua" && rm -f "$seed/cua.bak"
printf '#!/bin/sh\necho existing cua-spaces\n' >"$seed/cua-spaces"
chmod +x "$seed/cua-spaces"
inst PATH="$seed:$PATH" -- --pair K7QM-4XPB-2RHD
check "--pair with Cua installed: uses it, no download" sh -c "[ $status = 0 ] && grep -q 'already installed at $seed/cua' '$work/out' && grep -qx 'host setup --pair K7QM-4XPB-2RHD' '$home/cua.log' && ! grep -q 'manifest' '$work/out' && [ ! -e '$home/.local/bin/cua' ]"
check "--pair with the app installed: keeps it, no AppImage" sh -c "grep -q 'Cua Spaces is already installed' '$work/out' && [ ! -e '$home/.local/bin/cua-spaces' ]"
# Installing anyway writes the default bin dir (/usr/local/bin as root): only as a user.
if [ "$(id -u)" != 0 ]; then
    inst PATH="$seed:$PATH" CUA_INSTALL_REINSTALL=1 -- --pair K7QM-4XPB-2RHD
    check "--pair with CUA_INSTALL_REINSTALL=1: installs again" sh -c "[ $status = 0 ] && ! grep -q 'already installed' '$work/out' && [ -x '$home/.local/bin/cua' ] && '$home/.local/bin/cua' --version | grep -q $version"
    inst PATH="$seed:$PATH" -- --pair K7QM-4XPB-2RHD --version "$version"
    check "--pair --version: installs that release" sh -c "[ $status = 0 ] && ! grep -q 'already installed' '$work/out' && [ -x '$home/.local/bin/cua' ]"
    inst PATH="$seed:$PATH" -- --yes --cli-only --prefix "$work/rp1"
    check "without --pair an installed cua is not reused" sh -c "[ $status = 0 ] && ! grep -q 'already installed' '$work/out' && [ -x '$work/rp1/bin/cua' ]"
    # (A real cua at one of the fixed places would be reused: skip there.)
    if [ ! -x /usr/local/bin/cua ] && [ ! -x "/Applications/Cua Spaces.app/Contents/MacOS/cua" ] && [ ! -x "$HOME/Applications/Cua Spaces.app/Contents/MacOS/cua" ]; then
        old="$work/seed-old/bin"
        make_cua "$old" linux-x64
        sed -i.bak 's/--pair <CODE>  add this machine with a code/--no-such-flag/' "$old/cua" && rm -f "$old/cua.bak"
        inst PATH="$old:$PATH" -- --pair K7QM-4XPB-2RHD
        check "--pair with an old cua (no --pair): installs the current one" sh -c "[ $status = 0 ] && ! grep -q 'already installed' '$work/out' && [ -x '$home/.local/bin/cua' ]"
    fi
fi

# One quiet info line when signatures cannot be checked (neither minisign nor cosign).
sig2="$release/sig2"
mkdir -p "$sig2"
cp "$tag_dir"/cua-cli-* "$tag_dir"/cua-spaces-"$version"-linux-x64.AppImage "$sig2/"
for f in "$sig2"/cua-cli-*.tar.gz "$sig2"/*.AppImage; do printf 'sig\n' >"$f.minisig"; done
python3 "$manifest_tool" --component cli --version "$version" --dir "$sig2" --out "$sig2/cli.json"
python3 "$manifest_tool" --component app --version "$version" --dir "$sig2" --merge "$sig2/cli.json" --out "$sig2/release-artifacts.json"
inst CUA_INSTALL_MANIFEST_URL="$base/sig2/release-artifacts.json" -- --yes --no-onboarding --verbose --select spaces --prefix "$work/sg1"
check "unverifiable signatures: one --verbose info line, no warning" sh -c "[ $status = 0 ] && [ \"\$(grep -c 'release signatures were not checked' '$work/out')\" = 1 ] && ! grep -qi 'warning.*minisign' '$work/out'"
inst CUA_INSTALL_MANIFEST_URL="$base/sig2/release-artifacts.json" -- --yes --no-onboarding --select spaces --prefix "$work/sg3"
check "unverifiable signatures, default output: plain checksum line only" sh -c "[ $status = 0 ] && grep -q '(downloads checked)' '$work/out' && ! grep -qi 'cosign\|minisign\|signatures were not\|checksum' '$work/out'"
inst CUA_INSTALL_MANIFEST_URL="$base/sig2/release-artifacts.json" -- --yes --no-onboarding --select spaces --require-signature --prefix "$work/sg2"
check "--require-signature still fails without minisign or cosign" sh -c "[ $status != 0 ] && grep -q 'no verifiable signature' '$work/out'"
# Upgrade over a running older daemon: the new cua stops it (never a broad
# kill) and starts the new one. A fake old cua reports a running 0.0.9 daemon.
mkdir -p "$work/up/bin"
cat >"$work/oldcua" <<'EOF'
#!/bin/sh
if [ "${1:-}" = --version ]; then echo "cua 0.0.9 (old)"; exit 0; fi
if [ "${1:-} ${2:-}" = "daemon status" ]; then echo "cua daemon 0.0.9 (pid 4242) socket=/x loopback=http://127.0.0.1:1"; exit 0; fi
exit 0
EOF
cp "$work/oldcua" "$work/up/bin/cua"
chmod +x "$work/up/bin/cua"
cat >"$work/stage/linux-x64/cua" <<EOF
#!/bin/sh
if [ "\${1:-}" = --version ]; then echo "cua $version (linux-x64)"; exit 0; fi
echo "\$*" >>"\${CUA_FAKE_LOG:-/dev/null}"
EOF
tar -czf "$tag_dir/cua-cli-$version-linux-x64.tar.gz" -C "$work/stage/linux-x64" cua
python3 "$manifest_tool" --component cli --version "$version" --dir "$tag_dir" \
    --base-url "$base/cua-sdk-v$version" --out "$work/cli-abs.json" || exit 1
python3 "$manifest_tool" --component app --version "$version" --dir "$tag_dir" \
    --base-url "$base/cua-sdk-v$version" --merge "$work/cli-abs.json" \
    --out "$latest_dir/release-artifacts.json" || exit 1
inst -- --yes --no-onboarding --cli-only --prefix "$work/up"
check "upgrade over a running older daemon: stops it with the new cua, then starts the new one" sh -c "[ $status = 0 ] && grep -qx 'daemon stop' '$home/cua.log' && grep -qx 'daemon start' '$home/cua.log' && grep -q 'restarted the cua daemon (0.0.9 -> $version)' '$work/out'"
inst -- --yes --no-onboarding --cli-only --prefix "$work/up2"
check "fresh install (no daemon): no daemon stop or start" sh -c "[ $status = 0 ] && ! grep -q 'daemon' '$home/cua.log' 2>/dev/null"
mkdir -p "$work/up3/bin"
sed 's/0.0.9/1.2.3/g' "$work/oldcua" >"$work/up3/bin/cua"
chmod +x "$work/up3/bin/cua"
inst -- --yes --no-onboarding --cli-only --prefix "$work/up3"
check "same-version daemon is left alone" sh -c "[ $status = 0 ] && ! grep -q 'daemon' '$home/cua.log' 2>/dev/null"

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
    check "interactive: after sign-in, the closing line points at agents setup" sh -c "grep -q 'Signing in to Cua' '$work/out' && grep -q 'Next: run \"cua agents setup\" to connect your AI agents.' '$work/out' && ! grep -q 'Next: run \"cua auth login\"' '$work/out'"
    env -i PATH="$PATH" HOME="$home" SHELL=/bin/sh CUA_INSTALL_BASE_URL="$base" CUA_INSTALL_OS=linux \
        CUA_INSTALL_ARCH=x86_64 CUA_FAKE_LOG="$home/cua2.log" \
        script -qec "$sh_bin '$installer' --yes --cli-only --no-onboarding --prefix '$work/p14'" /dev/null >"$work/out" 2>&1 </dev/null
    check "interactive + --no-onboarding: no login" sh -c "! grep -qx 'auth login' '$home/cua2.log' 2>/dev/null"

    # PATH prompt on a terminal: Enter adds it to the profile, n leaves it.
    # `script` runs its command through $SHELL, so zsh (absent from the CI
    # containers) is only set inside the command, never as the outer shell.
    if command -v timeout >/dev/null 2>&1; then
        pty_path() { # KEYS PREFIX
            home="$work/home.pty.$RANDOM_SEQ"
            RANDOM_SEQ=$((RANDOM_SEQ + 1))
            mkdir -p "$home"
            (
                sleep 2
                printf '%b' "$1"
                sleep 2
            ) | env -i PATH="$PATH" HOME="$home" SHELL=/bin/sh CUA_HOME="$home/.cua" \
                CUA_INSTALL_BASE_URL="$base" CUA_INSTALL_OS=linux CUA_INSTALL_ARCH=x86_64 CUA_FAKE_LOG="$home/cua.log" \
                timeout 60 script -qec "env SHELL=/bin/zsh $sh_bin '$installer' --cli-only --no-onboarding --prefix '$2'" /dev/null >"$work/out" 2>&1
            status=$?
        }
        pty_path '\r\r' "$work/pt1"
        check "PATH prompt: Enter adds the bin dir to .zshrc" sh -c "[ $status = 0 ] && grep -q '$work/pt1/bin' '$home/.zshrc' && grep -q 'adds .* to PATH' '$work/out'"
        pty_path '\rn\r' "$work/pt2"
        check "PATH prompt: n leaves the profile alone and prints the one-liner" sh -c "[ $status = 0 ] && [ ! -e '$home/.zshrc' ] && grep -q 'is not on your PATH' '$work/out'"

        # End of input (Ctrl-D) is not an answer: the install question takes
        # its default, the PATH question leaves the profile alone.
        pty_eof() { # PREFIX
            home="$work/home.pty.$RANDOM_SEQ"
            RANDOM_SEQ=$((RANDOM_SEQ + 1))
            mkdir -p "$home"
            (
                sleep 2
                printf '\004'
                sleep 1
                printf '\004'
                sleep 2
            ) | env -i PATH="$PATH" HOME="$home" SHELL=/bin/sh CUA_HOME="$home/.cua" \
                CUA_INSTALL_BASE_URL="$base" CUA_INSTALL_OS=linux CUA_INSTALL_ARCH=x86_64 CUA_FAKE_LOG="$home/cua.log" \
                timeout 60 script -qec "env SHELL=/bin/zsh $sh_bin '$installer' --cli-only --no-onboarding --prefix '$1'" /dev/null >"$work/out" 2>&1
            status=$?
        }
        pty_eof "$work/pt3"
        check "prompts at end of input: install by default, never edit the profile" sh -c "[ $status = 0 ] && [ -x '$work/pt3/bin/cua' ] && [ ! -e '$home/.zshrc' ] && grep -q 'is not on your PATH' '$work/out' && ! grep -q 'Added .* to PATH' '$work/out'"
    fi

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
            timeout 60 script -qec "$sh_bin '$installer' --no-onboarding --no-modify-path $*" /dev/null >"$work/out" 2>&1
        status=$?
    }
    logged() { grep -q -- "$1" "$home/cua.log" 2>/dev/null; }
    if command -v bash >/dev/null 2>&1 && command -v timeout >/dev/null 2>&1; then
        tty_inst '\r' --prefix "$work/t1"
        check "checklist: says what Cua Spaces is and what is always installed" sh -c "grep -q 'Cua Spaces lets your AI agents use computers, and lets you watch them work.' '$work/out' && grep -q 'Needed, so always installed:' '$work/out' && grep -q 'Command-line tools: the \"cua\" program that the app and your AI agents use.' '$work/out' && grep -q 'Choose what else to add:' '$work/out'"
        check "checklist: Cua Spaces app and Cua Driver rows, with a line on the highlighted one" sh -c "grep -q '> \\[x\\] Cua Spaces app: lets your AI agents use computers' '$work/out' && grep -q '\\[ \\] Cua Driver: lets AI agents use your apps while you keep working' '$work/out' && grep -q 'Open the app to sign in, connect your AI agents and watch them work.' '$work/out'"
        check "checklist: no CLI row, no hosting row, short keyboard help" sh -c "! grep -q 'cua CLI (required)' '$work/out' && ! grep -qi 'host this machine' '$work/out' && grep -q 'Up/down to move, space to select, Enter to install' '$work/out'"
        check "checklist: Enter accepts the defaults (CLI and the app)" sh -c "[ $status = 0 ] && [ -e '$work/t1/bin/cua' ] && [ ! -e '$work/t1/bin/cua-driver' ] && [ -x '$work/t1/bin/cua-spaces' ]"
        check "checklist: no per-item prompt afterwards" sh -c "! grep -q 'Install the command-line tools to' '$work/out'"
        tty_inst '2\r' --prefix "$work/t2"
        check "checklist: number key toggles cua-driver on" sh -c "[ $status = 0 ] && [ -e '$work/t2/bin/cua-driver' ] && grep -q 'agents setup --cua-driver' '$home/cua.log'"
        tty_inst '\033[B \r' --prefix "$work/t3"
        check "checklist: arrow down + space select cua-driver" sh -c "[ $status = 0 ] && [ -e '$work/t3/bin/cua-driver' ] && ! grep -qx 'host setup' '$home/cua.log'"
        tty_inst '1\r' --prefix "$work/t4"
        check "checklist: 1 turns the app off; the CLI is always installed" sh -c "[ $status = 0 ] && [ -e '$work/t4/bin/cua' ] && [ ! -e '$work/t4/bin/cua-spaces' ] && [ ! -e '$work/t4/bin/cua-driver' ]"
        tty_inst '3\r' --prefix "$work/t4b"
        check "checklist: a number past the last row does nothing" sh -c "[ $status = 0 ] && [ -x '$work/t4b/bin/cua-spaces' ] && [ ! -e '$work/t4b/bin/cua-driver' ] && ! grep -qx 'host setup' '$home/cua.log'"
        tty_inst '\r' --select cua-driver --prefix "$work/t5"
        check "checklist: --select cua-driver is preselected" sh -c "[ $status = 0 ] && [ -e '$work/t5/bin/cua-driver' ] && grep -q '\\[x\\] Cua Driver' '$work/out'"
        tty_inst '2\r' --select cua-driver --prefix "$work/t6"
        check "checklist: a preselected item can be turned off" sh -c "[ $status = 0 ] && [ -e '$work/t6/bin/cua' ] && [ ! -e '$work/t6/bin/cua-driver' ]"
        tty_inst '\r' --select spaces --prefix "$work/t7"
        check "checklist: --select spaces on Linux: installed" sh -c "[ $status = 0 ] && grep -q 'Cua Spaces app' '$work/out' && [ -x '$work/t7/bin/cua-spaces' ] && [ -e '$work/t7/bin/cua' ]"
        tty_inst '' --prefix "$work/t8"
        check "checklist: no input (EOF) accepts defaults, no hang" sh -c "[ $status = 0 ] && [ -e '$work/t8/bin/cua' ]"
        tty_inst '\r' --select host --prefix "$work/t8b"
        check "checklist: --select host (a script flag) is not a row, but is said and honoured" sh -c "[ $status = 0 ] && grep -q 'Also: your other devices will be able to use this computer' '$work/out' && ! grep -qi 'host this machine' '$work/out' && grep -qx 'host setup' '$home/cua.log'"
        tty_inst '\r' --yes --select host --prefix "$work/t9"
        check "checklist: --yes skips it" sh -c "[ $status = 0 ] && ! grep -q 'Choose what to install' '$work/out' && grep -qx 'host setup' '$home/cua.log'"
        tty_inst '\r' --only cua-driver --prefix "$work/t10"
        check "checklist: --only skips it" sh -c "[ $status = 0 ] && ! grep -q 'Choose what to install' '$work/out' && [ -e '$work/t10/bin/cua-driver' ]"
        tty_term=dumb
        tty_inst '2\n\n' --prefix "$work/t11"
        tty_term=""
        check "checklist (TERM=dumb): numbered prompt with a line per row toggles cua-driver" sh -c "[ $status = 0 ] && grep -q 'Type a number to switch an item on or off' '$work/out' && grep -q 'For advanced users. Adds it to Claude Code, Codex and other coding agents.' '$work/out' && [ -e '$work/t11/bin/cua-driver' ] && ! grep -qx 'host setup' '$home/cua.log'"
        [ -n "${CUA_INSTALL_TRANSCRIPT:-}" ] && tty_inst '\033[B \r' --prefix "$work/t12" && cp "$work/out" "$CUA_INSTALL_TRANSCRIPT"
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
