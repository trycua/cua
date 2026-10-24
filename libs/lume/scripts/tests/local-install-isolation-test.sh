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
trap 'rm -rf "$TMP"' EXIT

H="$TMP/home"                 # the fake HOME
SRC="$TMP/src/lume"           # a copy of the script tree, so builds stay out of the checkout
BIN="$TMP/bin"                # the stubs
STUB_LOG_DIR="$TMP/stub-logs"
LOGS="$TMP/logs"              # stands in for /tmp
mkdir -p "$H" "$SRC/scripts" "$SRC/resources" "$BIN" "$STUB_LOG_DIR" "$LOGS"

cp "$REAL_LUME_DIR/scripts/install-local.sh" "$REAL_LUME_DIR/scripts/uninstall-local.sh" "$SRC/scripts/"
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
  *) echo "stub-lume name=$(basename "$0") args=$*" ;;
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
    "$LOGS/lume_daemon.error.log"
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
check "plist never names port 7777" bash -c "! grep -q 7777 '$PLIST'"
check "stable plist content is unchanged" grep -q 'stable daemon 7777' "$H/Library/LaunchAgents/com.trycua.lume_daemon.plist"

check "launchctl was called" test -s "$STUB_LOG_DIR/launchctl.log"
check "launchctl only saw the .local plist" bash -c "! grep -v 'com.trycua.lume_daemon.local.plist' '$STUB_LOG_DIR/launchctl.log'"
check "launchctl loaded the .local plist" grep -q "^launchctl load $PLIST\$" "$STUB_LOG_DIR/launchctl.log"
check "codesign used the local identifier" grep -q -- '--identifier com.trycua.lume.local' "$STUB_LOG_DIR/codesign.log"
check "codesign never touched the install dirs" bash -c "! grep -q '$H' '$STUB_LOG_DIR/codesign.log'"

WRAPPED="$("$H/.local/bin/lume-local" serve 2>&1)"
check "the wrapper runs the lume-local binary" test "$WRAPPED" = "stub-lume name=lume-local args=serve"

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
  check "bridged wrapper runs the app's lume-local" grep -q "exec \"$APP/Contents/MacOS/lume-local\"" "$H3/.local/bin/lume-local"
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
printf 'y\nn\n' | run uninstall-local.sh >/dev/null 2>&1
check "uninstall keeping VMs succeeds" test $? -eq 0
check "uninstall removed the command" test ! -e "$H/.local/bin/lume-local"
check "uninstall removed the files" test ! -e "$H/.local/share/lume-local"
check "uninstall removed the .local plist" test ! -e "$PLIST"
check "uninstall removed the .local logs" test ! -e "$LOGS/lume_daemon.local.log"
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
