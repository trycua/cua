#!/bin/bash
# Isolation test for libs/lume/scripts/install-local.sh and uninstall-local.sh.
#
# Runs both scripts against a fake HOME, with swift, codesign and launchctl
# replaced by stubs, so nothing is built, signed or loaded and the real home is
# never read. It then checks that the released Lume's files (~/.lume,
# ~/.local/bin/lume, ~/.local/share/lume, the com.trycua.lume_daemon plist and
# the config under ~/.config/lume) are byte-for-byte and mtime-for-mtime the
# same, that launchctl was only ever pointed at the `.local` plist, and that
# everything the scripts did write is a lume-local path.
#
# Usage: libs/lume/scripts/tests/local-install-isolation-test.sh
# Exit status is 0 when every check passes. macOS only (it reads launchd plists
# and uses BSD stat).

set -u -o pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REAL_LUME_DIR="$(cd "$HERE/../.." && pwd)"

if [ "$(id -u)" -eq 0 ]; then
  echo "SKIP: install-local.sh refuses to run as root, so it cannot be tested as root."
  exit 0
fi

TMP="$(mktemp -d "${TMPDIR:-/tmp}/lume-local-install-test.XXXXXX")"
TMP="$(cd -P "$TMP" && pwd -P)"   # the scripts print canonical paths; compare like with like
trap 'rm -rf "$TMP"' EXIT

H="$TMP/home"                 # the fake HOME
SRC="$TMP/src/lume"           # a copy of the script tree, so builds stay out of the checkout
BIN="$TMP/bin"                # the stubs
STUB_LOG_DIR="$TMP/stub-logs"
LOGS="$TMP/logs"              # stands in for /tmp
mkdir -p "$H" "$SRC/scripts" "$SRC/resources" "$BIN" "$STUB_LOG_DIR" "$LOGS"

cp "$REAL_LUME_DIR/scripts/install-local.sh" "$REAL_LUME_DIR/scripts/uninstall-local.sh" \
  "$REAL_LUME_DIR/scripts/_lume-local-common.sh" "$SRC/scripts/"
for f in lume.entitlements lume.local.entitlements Info.plist AppIcon.icns; do
  cp "$REAL_LUME_DIR/resources/$f" "$SRC/resources/$f"
done

# ---- stubs -----------------------------------------------------------------

cat > "$BIN/swift" <<'STUB'
#!/bin/sh
# Fake swift: "builds" a stub lume into .build/<config>.
config=debug
prev=
for a in "$@"; do
  if [ "$prev" = "-c" ] && [ "$a" = "release" ]; then config=release; fi
  prev=$a
done
echo "swift $*" >> "$STUB_LOG_DIR/swift.log"
mkdir -p ".build/$config/lume_lume.bundle"
echo preset > ".build/$config/lume_lume.bundle/presets.txt"
cat > ".build/$config/lume" <<'LUME'
#!/bin/sh
case "$1" in
  --version) echo "lume version 0.0.0-test" ;;
  *) echo "stub-lume name=$(basename "$0") args=$* LUME_HOME=${LUME_HOME-unset} LUME_LOCAL_LOG_DIR=${LUME_LOCAL_LOG_DIR-unset}" ;;
esac
LUME
chmod +x ".build/$config/lume"
STUB

cat > "$BIN/codesign" <<'STUB'
#!/bin/sh
echo "codesign $*" >> "$STUB_LOG_DIR/codesign.log"
STUB

cat > "$BIN/launchctl" <<'STUB'
#!/bin/sh
echo "launchctl $*" >> "$STUB_LOG_DIR/launchctl.log"
STUB
chmod +x "$BIN/swift" "$BIN/codesign" "$BIN/launchctl"

# ---- helpers ---------------------------------------------------------------

FAILS=0
CHECKS=0

check() { # description, command...
  local desc="$1"
  shift
  CHECKS=$((CHECKS + 1))
  if ! "$@" >/dev/null 2>&1; then
    FAILS=$((FAILS + 1))
    echo "FAIL: $desc"
  fi
}

same() { # description, before, after
  CHECKS=$((CHECKS + 1))
  if [ "$2" != "$3" ]; then
    FAILS=$((FAILS + 1))
    echo "FAIL: $1"
    diff <(printf '%s\n' "$2") <(printf '%s\n' "$3") | head -20
  fi
}

if stat -f %m / >/dev/null 2>&1; then
  mtime() { stat -f %m "$1"; }
else
  mtime() { stat -c %Y "$1"; }
fi

# Every entry under the given paths, with content hash and mtime for files.
fingerprint() {
  local p f
  for p in "$@"; do
    if [ ! -e "$p" ] && [ ! -L "$p" ]; then
      echo "MISSING $p"
      continue
    fi
    find "$p" -print0 | sort -z | while IFS= read -r -d '' f; do
      if [ -L "$f" ]; then
        echo "L $f -> $(readlink "$f")"
      elif [ -d "$f" ]; then
        echo "D $f"
      else
        echo "F $f $(shasum -a 256 < "$f" | cut -d' ' -f1) $(mtime "$f")"
      fi
    done
  done
}

# Every file and link under HOME, sorted: what exists, not what it says.
inventory() {
  (cd "$1" && find . \( -type f -o -type l \) | sort)
}

# The paths the released Lume owns.
stable_paths() {
  printf '%s\n' \
    "$1/.lume" \
    "$1/.config/lume" \
    "$1/.local/share/lume" \
    "$1/.local/bin/lume" \
    "$1/.local/bin/lume_lume.bundle" \
    "$1/Library/LaunchAgents/com.trycua.lume_daemon.plist" \
    "$LOGS/lume_daemon.log" \
    "$LOGS/lume_daemon.error.log" \
    "$1/Library/Logs/lume"
}

seed_stable() { # home
  local home="$1"
  mkdir -p "$home/.lume/vm1" "$home/.config/lume" "$home/.local/share/lume/lume.app/Contents/MacOS" \
    "$home/.local/bin/lume_lume.bundle/Contents" "$home/Library/LaunchAgents" "$LOGS"
  printf 'stable\n' > "$home/.lume/release-channel"
  printf '{"name":"vm1"}\n' > "$home/.lume/vm1/config.json"
  printf 'disk\n' > "$home/.lume/vm1/disk.img"
  printf 'defaultLocationName: "home"\n' > "$home/.config/lume/config.yaml"
  printf '#!/bin/sh\nexit 0\n' > "$home/.local/share/lume/lume.app/Contents/MacOS/lume"
  printf '#!/bin/sh\nexec stable\n' > "$home/.local/bin/lume"
  chmod +x "$home/.local/bin/lume"
  printf 'resources\n' > "$home/.local/bin/lume_lume.bundle/Contents/stable.txt"
  printf '<plist>stable daemon 7777</plist>\n' > "$home/Library/LaunchAgents/com.trycua.lume_daemon.plist"
  printf 'stable log\n' > "$LOGS/lume_daemon.log"
  printf 'stable err\n' > "$LOGS/lume_daemon.error.log"
  mkdir -p "$home/Library/Logs/lume"
  printf 'released detached run\n' > "$home/Library/Logs/lume/vm1.log"
  # An old mtime, so any write shows up as a change.
  find "$home" "$LOGS" -type f -exec touch -t 202001010000 {} +
}

# Run a script from the copied tree under the fake HOME, with only the stubs
# on PATH. Extra VAR=value arguments go before the script.
run() { # [VAR=value...] script args...
  local extra=()
  while [ "$#" -gt 0 ] && [[ "$1" == *=* ]]; do
    extra+=("$1")
    shift
  done
  local script="$1"
  shift
  env -i HOME="$H" PATH="$BIN:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb \
    STUB_LOG_DIR="$STUB_LOG_DIR" LUME_LOCAL_LOG_DIR="$LOGS" \
    ${extra[@]+"${extra[@]}"} \
    /bin/bash "$SRC/scripts/$script" "$@"
}

reset_logs() { rm -f "$STUB_LOG_DIR"/*.log; }

# Whether every path in $1 (a newline list, relative to HOME) is a lume-local path.
only_local_paths() {
  local p
  while IFS= read -r p; do
    [ -z "$p" ] && continue
    case "$p" in
      ./.local/bin/lume-local) ;;
      ./.local/share/lume-local/*) ;;
      ./.lume-local/*) ;;
      ./Library/LaunchAgents/com.trycua.lume_daemon.local.plist) ;;
      *) echo "unexpected path: $p" >&2; return 1 ;;
    esac
  done <<< "$1"
}

# The value after --port in a plist's ProgramArguments.
plist_port() {
  grep -A1 -x '        <string>--port</string>' "$1" | sed -n 's/.*<string>\(.*\)<\/string>.*/\1/p' | tail -1
}

# The value of an EnvironmentVariables key in a plist.
plist_env() { # plist, key
  grep -A1 "<key>$2</key>" "$1" | sed -n 's/.*<string>\(.*\)<\/string>.*/\1/p' | tail -1
}

PLIST="$H/Library/LaunchAgents/com.trycua.lume_daemon.local.plist"

# ---- 1. install into a home that already has the released Lume ---------------

seed_stable "$H"
STABLE_BEFORE="$(fingerprint $(stable_paths "$H"))"
INVENTORY_BEFORE="$(inventory "$H")"
reset_logs

OUT="$(run install-local.sh 2>&1)"
check "install-local.sh succeeds" test $? -eq 0
[ -n "${DEBUG_TEST_OUTPUT:-}" ] && echo "$OUT"

same "install leaves every released-Lume path untouched" \
  "$STABLE_BEFORE" "$(fingerprint $(stable_paths "$H"))"

ADDED="$(diff <(printf '%s\n' "$INVENTORY_BEFORE") <(inventory "$H") | sed -n 's/^> //p')"
REMOVED="$(diff <(printf '%s\n' "$INVENTORY_BEFORE") <(inventory "$H") | sed -n 's/^< //p')"
check "install removed nothing" test -z "$REMOVED"
check "install wrote only lume-local paths" only_local_paths "$ADDED"

check "lume-local command exists" test -x "$H/.local/bin/lume-local"
check "lume-local command is the managed wrapper" grep -q 'lume-local wrapper, managed by' "$H/.local/bin/lume-local"
check "lume-local binary is named lume-local" test -x "$H/.local/share/lume-local/lume-local"
check "resource bundle sits next to the binary" test -d "$H/.local/share/lume-local/lume_lume.bundle"
check "release-channel file is in the local home" grep -qx stable "$H/.lume-local/release-channel"
check "the plist is the .local plist" test -f "$PLIST"
check "plist label is com.trycua.lume_daemon.local" grep -q '<string>com.trycua.lume_daemon.local</string>' "$PLIST"
check "plist serves on 7787" test "$(plist_port "$PLIST")" = 7787
check "plist runs lume-local" grep -q "<string>$H/.local/bin/lume-local</string>" "$PLIST"
check "plist sets LUME_HOME to the local home" test "$(plist_env "$PLIST" LUME_HOME)" = "$H/.lume-local"
check "plist logs to the .local logs" grep -q "<string>$LOGS/lume_daemon.local.log</string>" "$PLIST"
check "plist tells the daemon where its logs are" test "$(plist_env "$PLIST" LUME_LOCAL_LOG_DIR)" = "$LOGS"
check "plist never names port 7777" bash -c "! grep -q 7777 '$PLIST'"
check "stable plist content is unchanged" grep -q 'stable daemon 7777' "$H/Library/LaunchAgents/com.trycua.lume_daemon.plist"

check "launchctl was called" test -s "$STUB_LOG_DIR/launchctl.log"
check "launchctl only saw the .local plist" bash -c "! grep -v 'com.trycua.lume_daemon.local.plist' '$STUB_LOG_DIR/launchctl.log'"
check "launchctl loaded the .local plist" grep -q "^launchctl load $PLIST\$" "$STUB_LOG_DIR/launchctl.log"
check "codesign used the local identifier" grep -q -- '--identifier com.trycua.lume.local' "$STUB_LOG_DIR/codesign.log"
check "codesign never touched the install dirs" bash -c "! grep -q '$H' '$STUB_LOG_DIR/codesign.log'"

# The wrapper's runtime environment: it runs the lume-local binary, and sets
# LUME_HOME to the Local home whatever the caller exported (typically the
# released Lume's).
wrapper() { # [VAR=value...] wrapper-path args...
  local extra=()
  while [ "$#" -gt 0 ] && [[ "$1" == *=* ]]; do
    extra+=("$1")
    shift
  done
  env -i PATH="$BIN:/usr/bin:/bin:/usr/sbin:/sbin" ${extra[@]+"${extra[@]}"} "$@"
}
check "the wrapper runs the lume-local binary with the Local home" test \
  "$(wrapper HOME="$H" "$H/.local/bin/lume-local" serve 2>&1)" = "stub-lume name=lume-local args=serve LUME_HOME=$H/.lume-local LUME_LOCAL_LOG_DIR=$LOGS"
check "the wrapper overrides an inherited released-Lume LUME_HOME" test \
  "$(wrapper HOME="$H" LUME_HOME="$H/.lume" "$H/.local/bin/lume-local" serve 2>&1)" = "stub-lume name=lume-local args=serve LUME_HOME=$H/.lume-local LUME_LOCAL_LOG_DIR=$LOGS"
check "the wrapper overrides an inherited LUME_HOME that points elsewhere" test \
  "$(wrapper HOME="$H" LUME_HOME=/somewhere/else "$H/.local/bin/lume-local" ls 2>&1)" = "stub-lume name=lume-local args=ls LUME_HOME=$H/.lume-local LUME_LOCAL_LOG_DIR=$LOGS"
check "the wrapper passes arguments through untouched" test \
  "$(wrapper HOME="$H" "$H/.local/bin/lume-local" run 'a b' --x=1 2>&1)" = "stub-lume name=lume-local args=run a b --x=1 LUME_HOME=$H/.lume-local LUME_LOCAL_LOG_DIR=$LOGS"

# ---- 2. install again, on another port: idempotent, still only .local --------

reset_logs
run install-local.sh --port 7790 >/dev/null 2>&1
check "second install succeeds" test $? -eq 0
same "second install leaves every released-Lume path untouched" \
  "$STABLE_BEFORE" "$(fingerprint $(stable_paths "$H"))"
check "second install moved the plist to the new port" test "$(plist_port "$PLIST")" = 7790
check "second install unloaded the old .local service" grep -q "^launchctl unload $PLIST\$" "$STUB_LOG_DIR/launchctl.log"
check "launchctl only saw the .local plist the second time" bash -c "! grep -v 'com.trycua.lume_daemon.local.plist' '$STUB_LOG_DIR/launchctl.log'"
check "the home's release-channel is not rewritten" grep -qx stable "$H/.lume-local/release-channel"

# ---- 3. refused settings write nothing ----------------------------------------

H2="$TMP/home2"
mkdir -p "$H2"
seed_stable "$H2"
S2_BEFORE="$(fingerprint $(stable_paths "$H2"))"
I2_BEFORE="$(inventory "$H2")"
reset_logs

H_SAVE="$H"
H="$H2"
run install-local.sh --port 7777 >/dev/null 2>&1
check "port 7777 is refused" test $? -ne 0
run install-local.sh --port abc >/dev/null 2>&1
check "a non-numeric port is refused" test $? -ne 0
run LUME_LOCAL_HOME="$H2/.lume" install-local.sh >/dev/null 2>&1
check "the released home is refused as the local home" test $? -ne 0
run LUME_LOCAL_HOME="$H2/.lume/local" install-local.sh >/dev/null 2>&1
check "a path inside the released home is refused" test $? -ne 0
run LUME_LOCAL_HOME="$H2" install-local.sh >/dev/null 2>&1
check "HOME itself is refused as the local home" test $? -ne 0
run LUME_LOCAL_HOME="relative/path" install-local.sh >/dev/null 2>&1
check "a relative local home is refused" test $? -ne 0
same "refused installs wrote nothing" "$I2_BEFORE" "$(inventory "$H2")"
same "refused installs left the released Lume alone" "$S2_BEFORE" "$(fingerprint $(stable_paths "$H2"))"
check "refused installs never ran swift" test ! -e "$STUB_LOG_DIR/swift.log"

# A LUME_HOME in the environment belongs to the released Lume: it is ignored.
run LUME_HOME="$TMP/not-ours" install-local.sh >/dev/null 2>&1
check "install with LUME_HOME set succeeds" test $? -eq 0
check "LUME_HOME in the environment is ignored" test ! -e "$TMP/not-ours"
check "the local home is still ~/.lume-local" test -f "$H2/.lume-local/release-channel"
same "install with LUME_HOME set left the released Lume alone" \
  "$S2_BEFORE" "$(fingerprint $(stable_paths "$H2"))"

# --no-background-service removes only the .local plist.
reset_logs
run install-local.sh --no-background-service >/dev/null 2>&1
check "install without a service succeeds" test $? -eq 0
check "the .local plist is gone" test ! -e "$H2/Library/LaunchAgents/com.trycua.lume_daemon.local.plist"
check "the stable plist stays" test -f "$H2/Library/LaunchAgents/com.trycua.lume_daemon.plist"
check "launchctl only saw the .local plist without a service" bash -c "! grep -v 'com.trycua.lume_daemon.local.plist' '$STUB_LOG_DIR/launchctl.log'"
same "install without a service left the released Lume alone" \
  "$S2_BEFORE" "$(fingerprint $(stable_paths "$H2"))"
H="$H_SAVE"

# ---- 3b. aliases of the released Lume's paths are refused ---------------------
#
# A raw-string guard accepts "$HOME/./.lume", "$HOME/x/../.lume" or a symlink to
# ~/.lume, and then `uninstall-local.sh --yes` deletes the released Lume's VMs.
# Every target is canonicalised first.

expect_refused() { # description, then the run() arguments
  local desc="$1" rc out
  shift
  reset_logs
  out="$(run "$@" 2>&1)"
  rc=$?
  # Exit status 2 is the guard's; anything else would be a failure for another reason.
  check "$desc is refused" test "$rc" -eq 2
  check "$desc says why" grep -q 'Error: ' <<< "$out"
  check "$desc ran no build and no launchctl" test ! -e "$STUB_LOG_DIR/swift.log" -a ! -e "$STUB_LOG_DIR/launchctl.log"
}

H4="$TMP/home4"
mkdir -p "$H4/work"
seed_stable "$H4"
ln -s "$H4/.lume" "$H4/alias-to-stable"                 # a symlink to the released home
ln -s "$H4" "$H4/home-link"                              # a symlink to HOME
ln -s "$H4/.local/share/lume" "$H4/share-alias"          # a symlink to the released app files
ln -s .lume "$H4/.lume-local"                            # the default Local home, already aliasing the released home
S4_BEFORE="$(fingerprint $(stable_paths "$H4"))"
I4_BEFORE="$(inventory "$H4")"
H_SAVE="$H"
H="$H4"

ALIASES=(
  "$H4/./.lume"
  "$H4/.lume/../.lume"
  "$H4/work/../.lume"
  "$H4/.lume/"
  "$H4/.lume//vm1"
  "$H4/.LUME"
  "$H4/alias-to-stable"
  "$H4/alias-to-stable/vm1"
  "$H4/alias-to-stable/../.lume"
  "$H4/home-link/.lume"
  "$H4/home-link/./work/../.lume/vm1"
  "$H4/home-link"
  "$H4/.local"
  "$H4/.local/share"
  "$H4/.local/share/lume"
  "$H4/share-alias/x"
  "$H4/.config"
  "$H4/.config/lume/sub"
  "$H4/Library"
  "$H4/Library/LaunchAgents/com.trycua.lume_daemon.plist"
  "$H4/Library/Logs/lume"
  "$H4/Library/Logs"
  "$H4/Library/Logs/lume/vm1.log"
  # Leading or trailing whitespace: the Swift runtime must never see a path
  # that the guard read as a different directory from the one it uses.
  "$H4/.lume "
  "$H4/.lume   "
  "$H4/.lume"$'\t'
  "$H4/.lume"$'\n'
  "$H4/.lume/vm1 "
  "$H4/alias-to-stable "
)
for alias_path in "${ALIASES[@]}"; do
  expect_refused "install with LUME_LOCAL_HOME=${alias_path#$H4}" LUME_LOCAL_HOME="$alias_path" install-local.sh
  expect_refused "uninstall --yes with LUME_LOCAL_HOME=${alias_path#$H4}" LUME_LOCAL_HOME="$alias_path" uninstall-local.sh --yes
done

# The default Local home is a symlink to the released home.
expect_refused "install when ~/.lume-local is a symlink to ~/.lume" install-local.sh
expect_refused "uninstall --yes when ~/.lume-local is a symlink to ~/.lume" uninstall-local.sh --yes

# The command, its files, its plist and its logs.
expect_refused "install with --install-dir at the released command" install-local.sh --install-dir "$H4/.local/bin/lume"
expect_refused "install with --install-dir inside the released home" install-local.sh --install-dir "$H4/.lume"
expect_refused "install with --install-dir through a symlink to the released home" install-local.sh --install-dir "$H4/alias-to-stable/vm1"
expect_refused "install with --install-dir at the released files" install-local.sh --install-dir "$H4/.local/share/lume"
expect_refused "uninstall --yes with --install-dir at the released command" uninstall-local.sh --yes --install-dir "$H4/.local/bin/lume"
expect_refused "install with the log directory inside the released home" LUME_LOCAL_LOG_DIR="$H4/alias-to-stable" LUME_LOCAL_HOME="$H4/safe-home" install-local.sh
expect_refused "install with whitespace around --install-dir" LUME_LOCAL_HOME="$H4/safe-home" install-local.sh --install-dir "$H4/bin "
expect_refused "uninstall --yes with whitespace around --install-dir" LUME_LOCAL_HOME="$H4/safe-home" uninstall-local.sh --yes --install-dir "$H4/bin "
expect_refused "install with whitespace around LUME_LOCAL_LOG_DIR" LUME_LOCAL_HOME="$H4/safe-home" LUME_LOCAL_LOG_DIR="$LOGS " install-local.sh
expect_refused "uninstall --yes with whitespace around LUME_LOCAL_LOG_DIR" LUME_LOCAL_HOME="$H4/safe-home" LUME_LOCAL_LOG_DIR="$LOGS "$'\t' uninstall-local.sh --yes
expect_refused "install with a relative LUME_LOCAL_HOME" LUME_LOCAL_HOME="relative/.lume" install-local.sh
expect_refused "install with a relative --install-dir" LUME_LOCAL_HOME="$H4/safe-home" install-local.sh --install-dir relative/bin

same "refused aliases wrote nothing" "$I4_BEFORE" "$(inventory "$H4")"
same "refused aliases left every released-Lume path untouched" "$S4_BEFORE" "$(fingerprint $(stable_paths "$H4"))"
check "the released VM is intact after every refused uninstall" test "$(cat "$H4/.lume/vm1/config.json")" = '{"name":"vm1"}'
check "the released home still holds its VM disk" test -f "$H4/.lume/vm1/disk.img"

# Symlinks planted where the installer writes.
H5="$TMP/home5"
mkdir -p "$H5"
seed_stable "$H5"
H="$H5"
ln -s ../share/lume "$H5/.local/share/lume-local"        # its files directory is the released one
S5_BEFORE="$(fingerprint $(stable_paths "$H5"))"
I5_BEFORE="$(inventory "$H5")"
expect_refused "install when the lume-local files directory is a symlink to the released one" install-local.sh
expect_refused "uninstall --yes when the lume-local files directory is a symlink to the released one" uninstall-local.sh --yes
rm "$H5/.local/share/lume-local"
ln -s com.trycua.lume_daemon.plist "$H5/Library/LaunchAgents/com.trycua.lume_daemon.local.plist"
I5_BEFORE="$(inventory "$H5")"
expect_refused "install when the .local plist is a symlink to the released plist" install-local.sh
expect_refused "uninstall --yes when the .local plist is a symlink to the released plist" uninstall-local.sh --yes
rm "$H5/Library/LaunchAgents/com.trycua.lume_daemon.local.plist"
ln -s lume "$H5/.local/bin/lume-local"                    # the command is a symlink to the released one
I5_BEFORE="$(inventory "$H5")"
expect_refused "install when lume-local is a symlink to the released lume" install-local.sh
expect_refused "uninstall --yes when lume-local is a symlink to the released lume" uninstall-local.sh --yes
rm "$H5/.local/bin/lume-local"
I5_BEFORE="$(inventory "$H5")"
rm -f "$LOGS/lume_daemon.local.log" "$LOGS/lume_daemon.local.error.log"
ln -s lume_daemon.log "$LOGS/lume_daemon.local.log"        # the daemon log is the released daemon's log
expect_refused "install when the .local daemon log is a symlink to the released log" install-local.sh
expect_refused "uninstall --yes when the .local daemon log is a symlink to the released log" uninstall-local.sh --yes
rm "$LOGS/lume_daemon.local.log"
ln -s lume_daemon.error.log "$LOGS/lume_daemon.local.error.log"
expect_refused "install when the .local daemon error log is a symlink to the released one" install-local.sh
expect_refused "uninstall --yes when the .local daemon error log is a symlink to the released one" uninstall-local.sh --yes
rm "$LOGS/lume_daemon.local.error.log"
ln -s lume "$H5/Library/Logs/lume-local"                  # the run-logs directory is the released one
I5_BEFORE="$(inventory "$H5")"
expect_refused "uninstall --yes when the lume-local run-logs directory is a symlink to the released one" uninstall-local.sh --yes
rm "$H5/Library/Logs/lume-local"
I5_BEFORE="$(inventory "$H5")"
same "planted symlinks: nothing was written" "$I5_BEFORE" "$(inventory "$H5")"
same "planted symlinks: the released Lume is untouched" "$S5_BEFORE" "$(fingerprint $(stable_paths "$H5"))"

# A custom Local home is canonicalised, reaches the daemon and the command, and
# is what an inherited LUME_HOME is replaced with.
H6="$TMP/home6"
mkdir -p "$H6/real-vms"
seed_stable "$H6"
ln -s real-vms "$H6/vms-link"
mkdir -p "$H6/real-logs"
ln -s real-logs "$H6/logs-link"
S6_BEFORE="$(fingerprint $(stable_paths "$H6"))"
H="$H6"
run LUME_LOCAL_HOME="$H6/./vms-link/../vms-link/local home" LUME_LOCAL_LOG_DIR="$H6/./logs-link/../logs-link/run logs" install-local.sh >/dev/null 2>&1
check "install with a custom home and log directory (through symlinks, with spaces) succeeds" test $? -eq 0
LOGS6="$H6/real-logs/run logs"
P6="$H6/Library/LaunchAgents/com.trycua.lume_daemon.local.plist"
check "the custom home is canonical in the plist" test "$(plist_env "$P6" LUME_HOME)" = "$H6/real-vms/local home"
check "the custom home exists and has its release-channel" test -f "$H6/real-vms/local home/release-channel"
check "no default Local home was made beside it" test ! -e "$H6/.lume-local"
# The configured log directory is persisted for the readers: the daemon gets it
# (so the logs API reads where it writes) and so does the command (`lume-local logs`).
check "the custom log directory is canonical in the plist's environment" test "$(plist_env "$P6" LUME_LOCAL_LOG_DIR)" = "$LOGS6"
check "the plist writes the daemon log in the custom directory" grep -q "<string>$LOGS6/lume_daemon.local.log</string>" "$P6"
check "the plist writes the daemon error log in the custom directory" grep -q "<string>$LOGS6/lume_daemon.local.error.log</string>" "$P6"
check "the daemon logs exist in the custom directory" test -f "$LOGS6/lume_daemon.local.log" -a -f "$LOGS6/lume_daemon.local.error.log"
check "the wrapper overrides an inherited LUME_LOCAL_LOG_DIR with the configured one" test \
  "$(wrapper HOME="$H6" LUME_LOCAL_LOG_DIR=/somewhere/else "$H6/.local/bin/lume-local" logs 2>&1)" = "stub-lume name=lume-local args=logs LUME_HOME=$H6/real-vms/local home LUME_LOCAL_LOG_DIR=$LOGS6"
check "the wrapper uses the custom home, not an inherited released-Lume LUME_HOME" test \
  "$(wrapper HOME="$H6" LUME_HOME="$H6/.lume" "$H6/.local/bin/lume-local" ls 2>&1)" = "stub-lume name=lume-local args=ls LUME_HOME=$H6/real-vms/local home LUME_LOCAL_LOG_DIR=$LOGS6"
check "the wrapper uses the custom home when LUME_HOME is unset" test \
  "$(wrapper HOME="$H6" "$H6/.local/bin/lume-local" ls 2>&1)" = "stub-lume name=lume-local args=ls LUME_HOME=$H6/real-vms/local home LUME_LOCAL_LOG_DIR=$LOGS6"
same "install with a custom home left the released Lume alone" "$S6_BEFORE" "$(fingerprint $(stable_paths "$H6"))"
mkdir -p "$H6/real-vms/local home/vm-x"
printf '{}\n' > "$H6/real-vms/local home/vm-x/config.json"
run LUME_LOCAL_HOME="$H6/vms-link/local home" LUME_LOCAL_LOG_DIR="$H6/logs-link/run logs" uninstall-local.sh --yes >/dev/null 2>&1
check "uninstall --yes with the same custom home succeeds" test $? -eq 0
check "uninstall removed the daemon logs from the custom directory" test ! -e "$LOGS6/lume_daemon.local.log" -a ! -e "$LOGS6/lume_daemon.local.error.log"
check "uninstall removed the custom home" test ! -e "$H6/real-vms/local home"
check "uninstall kept the directory that held it" test -d "$H6/real-vms"
same "uninstall of the custom home left the released Lume alone" "$S6_BEFORE" "$(fingerprint $(stable_paths "$H6"))"
H="$H_SAVE"

# ---- 4. bridged-entitlement build: an .app named lume-local -------------------

if [ -x /usr/libexec/PlistBuddy ]; then
  H3="$TMP/home3"
  mkdir -p "$H3"
  seed_stable "$H3"
  S3_BEFORE="$(fingerprint $(stable_paths "$H3"))"
  reset_logs
  H="$H3"
  run install-local.sh --bridged-entitlement --no-background-service >/dev/null 2>&1
  check "bridged install succeeds" test $? -eq 0
  APP="$H3/.local/share/lume-local/lume-local.app"
  check "bridged install makes lume-local.app" test -x "$APP/Contents/MacOS/lume-local"
  check "bridged app has the local bundle id" test \
    "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP/Contents/Info.plist")" = com.trycua.lume.local
  check "bridged app executable is lume-local" test \
    "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$APP/Contents/Info.plist")" = lume-local
  check "bridged wrapper runs the app's lume-local with the Local home" test \
    "$(wrapper HOME="$H3" LUME_HOME="$H3/.lume" "$H3/.local/bin/lume-local" ls 2>&1)" = "stub-lume name=lume-local args=ls LUME_HOME=$H3/.lume-local LUME_LOCAL_LOG_DIR=$LOGS"
  check "bridged install leaves no stale standalone binary" test ! -e "$H3/.local/share/lume-local/lume-local"
  same "bridged install left the released Lume alone" \
    "$S3_BEFORE" "$(fingerprint $(stable_paths "$H3"))"
  H="$H_SAVE"
fi

# ---- 5. uninstall -------------------------------------------------------------

# 5a. Answering no removes nothing.
INV_INSTALLED="$(inventory "$H")"
reset_logs
printf 'n\n' | run uninstall-local.sh >/dev/null 2>&1
check "uninstall answered no succeeds" test $? -eq 0
same "uninstall answered no removes nothing" "$INV_INSTALLED" "$(inventory "$H")"
check "uninstall answered no never called launchctl" test ! -e "$STUB_LOG_DIR/launchctl.log"

# 5b. Yes to removal, no to deleting the VMs: the home and its VMs stay.
mkdir -p "$H/.lume-local/vm-local"
printf '{"name":"vm-local"}\n' > "$H/.lume-local/vm-local/config.json"
printf 'disk\n' > "$H/.lume-local/vm-local/disk.img"
reset_logs
# Logs of a detached `lume-local run`: lume-local's own directory, beside (not in) the released Lume's.
mkdir -p "$H/Library/Logs/lume-local"
printf 'local detached run\n' > "$H/Library/Logs/lume-local/vm-local.log"
check "the released Lume's detached-run log is in a different directory" test "$(cat "$H/Library/Logs/lume/vm1.log")" = "released detached run"
printf 'y\nn\n' | run uninstall-local.sh >/dev/null 2>&1
check "uninstall keeping VMs succeeds" test $? -eq 0
check "uninstall removed the command" test ! -e "$H/.local/bin/lume-local"
check "uninstall removed the files" test ! -e "$H/.local/share/lume-local"
check "uninstall removed the .local plist" test ! -e "$PLIST"
check "uninstall removed the .local logs" test ! -e "$LOGS/lume_daemon.local.log"
check "uninstall removed lume-local's detached-run logs" test ! -e "$H/Library/Logs/lume-local"
check "uninstall kept the released Lume's detached-run logs" test -f "$H/Library/Logs/lume/vm1.log"
check "uninstall kept the local VMs" test -f "$H/.lume-local/vm-local/config.json"
check "launchctl only saw the .local plist on uninstall" bash -c "! grep -v 'com.trycua.lume_daemon.local.plist' '$STUB_LOG_DIR/launchctl.log'"
same "uninstall left every released-Lume path untouched" \
  "$STABLE_BEFORE" "$(fingerprint $(stable_paths "$H"))"

# 5c. --yes deletes everything local, VMs included.
reset_logs
run install-local.sh >/dev/null 2>&1
check "reinstall after uninstall succeeds" test $? -eq 0
check "reinstall found the kept VMs" test -f "$H/.lume-local/vm-local/config.json"
reset_logs
run uninstall-local.sh --yes >/dev/null 2>&1
check "uninstall --yes succeeds" test $? -eq 0
check "uninstall --yes deleted the local home" test ! -e "$H/.lume-local"
same "after uninstall --yes only the released Lume's files remain" \
  "$INVENTORY_BEFORE" "$(inventory "$H")"
same "uninstall --yes left every released-Lume path untouched" \
  "$STABLE_BEFORE" "$(fingerprint $(stable_paths "$H"))"
check "launchctl only saw the .local plist on uninstall --yes" bash -c "! grep -v 'com.trycua.lume_daemon.local.plist' '$STUB_LOG_DIR/launchctl.log'"

# 5d. --yes --keep-vms keeps the home.
run install-local.sh >/dev/null 2>&1
mkdir -p "$H/.lume-local/vm-keep"
printf '{}\n' > "$H/.lume-local/vm-keep/config.json"
run uninstall-local.sh --yes --keep-vms >/dev/null 2>&1
check "uninstall --yes --keep-vms succeeds" test $? -eq 0
check "uninstall --yes --keep-vms kept the VMs" test -f "$H/.lume-local/vm-keep/config.json"
check "uninstall --yes --keep-vms removed the command" test ! -e "$H/.local/bin/lume-local"
rm -rf "$H/.lume-local"

# 5e. A file named lume-local that is not ours is left alone.
printf '#!/bin/sh\necho mine\n' > "$H/.local/bin/lume-local"
run uninstall-local.sh --yes >/dev/null 2>&1
check "uninstall with a foreign lume-local succeeds" test $? -eq 0
check "uninstall leaves a lume-local it did not install" grep -q mine "$H/.local/bin/lume-local"
rm -f "$H/.local/bin/lume-local"

# 5f. Uninstall refuses the released home.
run LUME_LOCAL_HOME="$H/.lume" uninstall-local.sh --yes >/dev/null 2>&1
check "uninstall refuses the released home" test $? -ne 0
same "uninstall of the released home removed nothing" \
  "$STABLE_BEFORE" "$(fingerprint $(stable_paths "$H"))"

# ---- result -------------------------------------------------------------------

if [ "$FAILS" -ne 0 ]; then
  echo "$FAILS of $CHECKS checks failed."
  exit 1
fi
echo "All $CHECKS checks passed."
