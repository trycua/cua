#!/bin/zsh
# Amendment 9 (CUA-1241): install arc-driver (arc-cua 0.1.1) for arm cc-arc-driver. VM ONLY: never run this on a
# Mac that is not a disposable bench VM, and take a VM clone first (A9.2). Run as the bench user, from this folder:
#
#   zsh install_arc_driver.sh            # venv + launcher app + TCC grants, then prints the values for pins.json
#
# What it does:
#   1. a Python 3.12 venv at $CDB_BENCH_WORK/arc-cua/venv with exactly the wheels of requirements-arc-0.1.1.txt
#      (hash-checked, binary wheels only, no dependency resolution, nothing built);
#   2. ArcDriverBench.app (bundle id com.trycua.bench.arcdriver): arc_launch.c compiled with clang, ad-hoc signed;
#   3. Accessibility and Screen Recording for that bundle id only (tcc_grant_arc.sh);
#   4. an empty HOME for the server at $CDB_BENCH_WORK/arc-cua/home.
set -euo pipefail
HERE=${0:A:h}
W=${CDB_BENCH_WORK:-$HOME/bench-work}
D=$W/arc-cua
APP=$D/ArcDriverBench.app
ID=com.trycua.bench.arcdriver
REQ=$HERE/requirements-arc-0.1.1.txt
UV=$(command -v uv || echo $HOME/.local/bin/uv)
[ -x "$UV" ] || { echo "uv not found" >&2; exit 1; }

mkdir -p $D/home
rm -rf $D/venv
$UV venv --quiet --python 3.12 $D/venv
$UV pip install --quiet --python $D/venv/bin/python --require-hashes --only-binary :all: --no-deps -r $REQ
# no bytecode in the pinned tree (the server runs with -B as well)
find $D/venv -name __pycache__ -type d -prune -exec rm -rf {} +

rm -rf $APP
mkdir -p $APP/Contents/MacOS
clang -O2 -Wall -o $APP/Contents/MacOS/arc-launch $HERE/arc_launch.c
cat > $APP/Contents/Info.plist <<PL
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>$ID</string>
<key>CFBundleName</key><string>Arc Driver Bench</string>
<key>CFBundleDisplayName</key><string>Arc Driver Bench</string>
<key>CFBundleExecutable</key><string>arc-launch</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.1</string>
<key>CFBundleVersion</key><string>0.1.1</string>
<key>LSUIElement</key><true/>
</dict></plist>
PL
codesign --force --sign - --identifier $ID $APP
codesign --verify --strict $APP && echo signed-ok
zsh $HERE/tcc_grant_arc.sh $APP $ID

echo "--- values for pins.json arc_driver (check them against the host-side values in the amendment)"
cd ${HERE:h:h}
CDB_BENCH_WORK=$W /opt/homebrew/bin/python3 -c "import json, claude_arms as ca; print(json.dumps(ca.arc_observed(), indent=2))"
