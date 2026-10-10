#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Proves "Cua Spaces.app" launches, without showing anything:
#
#  1. every Mach-O in the bundle (the app, the bundled `cua`, the SDK
#     library, Sparkle and its helpers) links only the OS and files inside
#     the bundle: each @rpath / @executable_path / @loader_path dependency
#     resolves, through the binary's own run paths, to a file in the bundle;
#  2. library validation will pass: a binary signed with the hardened runtime
#     (and without com.apple.security.cs.disable-library-validation) loads
#     only libraries of its own team, and an ad hoc signature has no team,
#     so an ad hoc hardened app cannot load an ad hoc framework (dyld: "have
#     different Team IDs"). Checked from the signatures, before anything runs;
#  3. `Contents/MacOS/CuaSpacesMac --check-launch-only` exits 0: dyld loaded
#     every linked library and their signatures passed library validation
#     (the hardened runtime's same-team rule), then main returned before any
#     window, daemon or updater.
#
#   scripts/check-launch.sh "path/Cua Spaces.app"
set -euo pipefail
app="${1:?usage: check-launch.sh <path/Cua Spaces.app>}"
[ -x "$app/Contents/MacOS/CuaSpacesMac" ] || { echo "$app has no Contents/MacOS/CuaSpacesMac" >&2; exit 1; }

python3 - "$app" <<'PY'
import os, subprocess, sys
app = os.path.realpath(sys.argv[1])
macos = os.path.join(app, "Contents", "MacOS")

def macho(path):
    try:
        with open(path, "rb") as f:
            magic = f.read(4)
    except OSError:
        return False
    return magic in (b"\xcf\xfa\xed\xfe", b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca", b"\xfe\xed\xfa\xcf")

def otool(*args):
    return subprocess.run(["otool", *args], capture_output=True, text=True, check=True).stdout

binaries = []
for root, dirs, files in os.walk(app):
    for name in files:
        p = os.path.join(root, name)
        if not os.path.islink(p) and macho(p):
            binaries.append(p)

bad = []
loads = {}
for b in sorted(binaries):
    executable = "EXECUTE" in otool("-hv", b)
    exe_dir = os.path.dirname(b) if executable else macos
    loader = os.path.dirname(b)
    def expand(p):
        return p.replace("@executable_path", exe_dir).replace("@loader_path", loader)
    rpaths, lines = [], otool("-l", b).splitlines()
    for i, line in enumerate(lines):
        if "cmd LC_RPATH" in line:
            rpaths.append(expand(lines[i + 2].split()[1]))
    # A library lists its own install name first; that is not a dependency.
    own = {l.strip() for l in otool("-D", b).splitlines()[1:] if l.strip()}
    deps = [l.split()[0] for l in otool("-L", b).splitlines()[1:] if l.startswith("\t")]
    deps = [d for d in deps if d not in own]
    for d in sorted(set(deps)):
        if d.startswith(("/usr/lib/", "/System/Library/")):
            continue
        if d.startswith("@rpath/"):
            candidates = [os.path.join(r, d[len("@rpath/"):]) for r in rpaths]
        elif d.startswith(("@executable_path", "@loader_path")):
            candidates = [expand(d)]
        else:
            bad.append(f"{os.path.relpath(b, app)} links {d} (outside the bundle and the OS)")
            continue
        found = [c for c in candidates if os.path.isfile(c)]
        if not found:
            bad.append(f"{os.path.relpath(b, app)}: {d} resolves to no file (run paths {rpaths})")
        elif not os.path.realpath(found[0]).startswith(app + os.sep):
            bad.append(f"{os.path.relpath(b, app)}: {d} resolves outside the bundle ({found[0]})")
        elif executable:
            loads.setdefault(b, []).append(os.path.realpath(found[0]))

def signature(path):
    info = subprocess.run(["codesign", "-dv", path], capture_output=True, text=True).stderr
    team = next((l.split("=", 1)[1] for l in info.splitlines() if l.startswith("TeamIdentifier=")), "not set")
    runtime = any(l.startswith("CodeDirectory") and "runtime" in l for l in info.splitlines())
    ents = subprocess.run(["codesign", "-d", "--entitlements", "-", "--xml", path],
                          capture_output=True, text=True).stdout
    return team, runtime, "com.apple.security.cs.disable-library-validation" in ents

# Library validation, from the signatures.
for exe, libs in loads.items():
    team, runtime, relaxed = signature(exe)
    if not runtime or relaxed:
        continue
    for lib in sorted(set(libs)):
        lib_team = signature(lib)[0]
        if team == "not set" or lib_team != team:
            bad.append(f"{os.path.relpath(exe, app)} (hardened, team {team}) cannot load "
                       f"{os.path.relpath(lib, app)} (team {lib_team}): library validation")
for line in bad:
    print(line, file=sys.stderr)
if bad:
    sys.exit(1)
print(f"{len(binaries)} binaries link only the OS and the bundle")
PY

# Only a build that knows --check-launch-only is run (an older one would start
# for real), and even then with a throwaway HOME and CUA_HOME, telemetry
# off and no inherited environment: nothing it could read or write is the
# account's.
# (grep reads everything: with -q, strings would die of SIGPIPE under
# pipefail.)
if ! strings -a "$app/Contents/MacOS/CuaSpacesMac" | grep -x -- '--check-launch-only' >/dev/null; then
  echo "CuaSpacesMac has no --check-launch-only (an older build); not running it" >&2
  exit 1
fi
out="$(mktemp)"
sandbox="$(mktemp -d)"
trap 'rm -f "$out"; rm -rf "$sandbox"' EXIT
env -i HOME="$sandbox" CUA_HOME="$sandbox/.cua" TMPDIR="$sandbox" PATH=/usr/bin:/bin \
  CUA_TELEMETRY=0 DO_NOT_TRACK=1 CUA_DAEMON_AUTOSTART=0 \
  "$app/Contents/MacOS/CuaSpacesMac" --check-launch-only >"$out" 2>&1 &
pid=$!
for _ in $(seq 1 100); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
if kill -0 "$pid" 2>/dev/null; then
  kill -9 "$pid" 2>/dev/null
  echo "CuaSpacesMac --check-launch-only did not exit within 10 s" >&2
  exit 1
fi
if wait "$pid" && grep -q ' loads$' "$out"; then
  cat "$out"
else
  echo "CuaSpacesMac does not launch:" >&2
  cat "$out" >&2
  exit 1
fi
