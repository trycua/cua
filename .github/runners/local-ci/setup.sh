#!/bin/bash
# Set up the `cua-local-ci` self-hosted GitHub Actions runners on a Mac (run as root, idempotent).
# They run ci-local-signoff.yml: `bin/ci --quick` for pull requests from branches of trycua/cua.
#
#   sudo setup.sh <runner-version> <sha256 of actions-runner-osx-arm64-<version>.tar.gz> <instances> <name-prefix> [<token-file>]
#
# <token-file> holds a repository runner registration token (POST repos/trycua/cua/actions/runners/registration-token,
# repository admin; it expires in an hour). Without it, the runners are installed but not registered or started.
# The file is deleted after use.
#
# Security model (trycua/cua is public):
#  - The runners run as `ghrunner`: a hidden, password-less, non-admin user with its own primary group, not in the
#    ssh or screen-sharing groups, with no GitHub or cloud credentials on disk. Jobs get only the job token.
#  - Label `cua-local-ci` only (--no-default-labels), so workflows asking for `self-hosted` never land here.
#  - job-started.sh (root-owned, outside the runner directory) refuses every job that is not ci-local-signoff.yml
#    on a pull_request whose head is a trycua/cua branch. A fork cannot run code here even by editing the workflow.
#  - The work directory is emptied after every job and at service start (job-completed.sh, run-instance.sh).
#  - The runners are persistent, not ephemeral: re-registering per job needs a repository-admin credential stored
#    on this machine, which would be a bigger risk than the cleaned, guarded persistent runner.
set -euo pipefail

version=${1:?runner version, e.g. 2.338.2}
sha=${2:?sha256 of the osx-arm64 tarball}
count=${3:?number of runner instances (1-9)}
prefix=${4:?runner name prefix}
tokenfile=${5:-}
[ "$(id -u)" = 0 ] || { echo "run as root" >&2; exit 1; }
case "$count" in [1-9]) ;; *) echo "instances must be 1-9" >&2; exit 1 ;; esac

USER_NAME=ghrunner
USER_ID=601
HOME_DIR=/Users/ghrunner
LIBEXEC=/usr/local/libexec/cua-local-ci
LABEL=cua-local-ci
PATH_FOR_JOBS="$HOME_DIR/.cargo/bin:/opt/homebrew/opt/node@22/bin:/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
here=$(cd "$(dirname "$0")" && pwd)

# 1. The unprivileged user, with its own group (not staff).
if ! dscl . -read /Groups/$USER_NAME >/dev/null 2>&1; then
  dscl . -create /Groups/$USER_NAME
  dscl . -create /Groups/$USER_NAME PrimaryGroupID $USER_ID
  dscl . -create /Groups/$USER_NAME RealName "GitHub Actions runner"
fi
if ! id $USER_NAME >/dev/null 2>&1; then
  dscl . -create /Users/$USER_NAME
  dscl . -create /Users/$USER_NAME UniqueID $USER_ID
  dscl . -create /Users/$USER_NAME PrimaryGroupID $USER_ID
  dscl . -create /Users/$USER_NAME RealName "GitHub Actions runner"
  dscl . -create /Users/$USER_NAME UserShell /bin/zsh
  dscl . -create /Users/$USER_NAME NFSHomeDirectory $HOME_DIR
  dscl . -create /Users/$USER_NAME Password '*'
  dscl . -create /Users/$USER_NAME IsHidden 1
fi
for g in admin com.apple.access_ssh com.apple.access_screensharing; do
  if dseditgroup -o checkmember -m $USER_NAME $g >/dev/null 2>&1; then echo "$USER_NAME must not be in $g" >&2; exit 1; fi
done
mkdir -p $HOME_DIR
chown $USER_NAME:$USER_NAME $HOME_DIR
chmod 700 $HOME_DIR
cd $HOME_DIR  # the runner user cannot read the caller's working directory

# 2. The hooks and the launcher: root-owned, outside anything the runner user can write.
install -d -o root -g wheel -m 755 $LIBEXEC
for f in job-started.sh job-completed.sh run-instance.sh; do
  install -o root -g wheel -m 755 "$here/$f" $LIBEXEC/$f
done

# 3. Toolchain the quick checks need that is not system-wide: Rust (rustfmt) for this user.
if [ ! -x $HOME_DIR/.cargo/bin/rustup ]; then
  sudo -u $USER_NAME -H /bin/bash -c 'curl -fsSL https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal -c rustfmt -c clippy' >/dev/null
fi

# 4. The runner itself, checksum-verified.
tarball=$HOME_DIR/actions-runner-osx-arm64-$version.tar.gz
if [ ! -f "$tarball" ]; then
  sudo -u $USER_NAME curl -fsSL -o "$tarball" "https://github.com/actions/runner/releases/download/v$version/actions-runner-osx-arm64-$version.tar.gz"
fi
echo "$sha  $tarball" | shasum -a 256 -c - >/dev/null || { echo "checksum mismatch for $tarball" >&2; exit 1; }

for n in $(seq 1 "$count"); do
  dir=$HOME_DIR/runner-$n
  plist=/Library/LaunchDaemons/com.trycua.local-ci-runner-$n.plist
  if [ ! -x "$dir/run.sh" ]; then
    sudo -u $USER_NAME mkdir -p "$dir"
    sudo -u $USER_NAME tar xzf "$tarball" -C "$dir"
  fi
  if [ -n "$tokenfile" ] && [ ! -f "$dir/.runner" ]; then
    sudo -u $USER_NAME -H env PATH="$PATH_FOR_JOBS" /bin/bash -c 'cd "$1" && ./config.sh --unattended --replace \
      --url https://github.com/trycua/cua --token "$(cat "$2")" --name "$3" --labels "$4" --no-default-labels --work _work' \
      _ "$dir" "$tokenfile" "$prefix-$n" "$LABEL"
  fi
  cat > "$plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.trycua.local-ci-runner-$n</string>
  <key>UserName</key><string>$USER_NAME</string>
  <key>GroupName</key><string>$USER_NAME</string>
  <key>ProgramArguments</key><array><string>$LIBEXEC/run-instance.sh</string><string>$n</string></array>
  <key>WorkingDirectory</key><string>$dir</string>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>30</integer>
  <key>ProcessType</key><string>Background</string>
  <key>SoftResourceLimits</key><dict><key>NumberOfFiles</key><integer>10240</integer></dict>
  <key>EnvironmentVariables</key>
  <dict>
    <key>HOME</key><string>$HOME_DIR</string>
    <key>PATH</key><string>$PATH_FOR_JOBS</string>
    <key>CUA_RUNNER_DIR</key><string>$dir</string>
    <key>ACTIONS_RUNNER_HOOK_JOB_STARTED</key><string>$LIBEXEC/job-started.sh</string>
    <key>ACTIONS_RUNNER_HOOK_JOB_COMPLETED</key><string>$LIBEXEC/job-completed.sh</string>
    <key>HOMEBREW_NO_AUTO_UPDATE</key><string>1</string>
  </dict>
  <key>StandardOutPath</key><string>$HOME_DIR/runner-$n.log</string>
  <key>StandardErrorPath</key><string>$HOME_DIR/runner-$n.log</string>
</dict>
</plist>
PLIST
  chown root:wheel "$plist"; chmod 644 "$plist"
  if [ -f "$dir/.runner" ]; then
    launchctl bootout system "$plist" 2>/dev/null || true
    launchctl bootstrap system "$plist"
    echo "runner $prefix-$n: started (launchd com.trycua.local-ci-runner-$n)"
  else
    echo "runner $prefix-$n: installed, not registered (no token)"
  fi
done
[ -n "$tokenfile" ] && rm -f "$tokenfile"
echo "done"
