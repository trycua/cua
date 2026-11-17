#!/usr/bin/env bash
# Offline test for attest-doctor-reports.sh's ledger_push: two jobs record
# into clones of the same ledger and push; the one that loses the race
# replays its entries onto the pushed ledger (lane by lane) and pushes again.
# Uses a local bare repository; no network.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/cua-e2e-ledger-race.XXXXXX")"
trap 'rm -rf "$work"' EXIT
export GIT_CONFIG_GLOBAL="$work/gitconfig" GIT_CONFIG_NOSYSTEM=1
git config --global init.defaultBranch main
git init -q --bare "$work/origin.git"
git clone -q "$work/origin.git" "$work/seed" 2>/dev/null
(cd "$work/seed" && printf '# ledger\n' >README.md && git add README.md &&
    git -c user.name=t -c user.email=t@t commit -qm init && git push -q origin HEAD:image-doctor-ledger)
# The function under test, extracted from the script (it needs only HERE,
# LEDGER_BRANCH and git).
eval "$(sed -n '/^ledger_push() {/,/^}/p' "$HERE/attest-doctor-reports.sh")"
LEDGER_BRANCH=image-doctor-ledger
clone() { git clone -q -b image-doctor-ledger "$work/origin.git" "$work/$1" 2>/dev/null; }
clone a; clone b
report() { # RUNTIME STATUS
    printf '{"schema_version":1,"spacesd":{"version":"0"},"image":{"variant":"rootfs","os":"linux"},"environment":{"runtime":"%s","arch":"amd64"},"summary":{"status":"%s","pass":1,"warn":0,"fail":0,"skip":0,"strict":true}}' "$1" "$2"
}
d1="sha256:$(printf '1%.0s' $(seq 1 64))"
report gvisor pass >"$work/r1.json"; report container pass >"$work/r2.json"
python3 "$HERE/doctor_ledger.py" record --ledger "$work/a" --repo ghcr.io/x/linux --digest "$d1" --report "$work/r1.json" --run job-a >/dev/null
python3 "$HERE/doctor_ledger.py" record --ledger "$work/b" --repo ghcr.io/x/linux --digest "$d1" --report "$work/r2.json" --run job-b >/dev/null
ledger_push "$work/a" "doctor: a"
ledger_push "$work/b" "doctor: b" 2>"$work/b.err"
grep -q "raced" "$work/b.err" || { echo "FAIL: b did not race"; exit 1; }
clone check
entry="$work/check/ghcr.io/x/linux/$(printf '1%.0s' $(seq 1 64)).json"
lanes="$(python3 -c 'import json,sys;print(",".join(sorted(json.load(open(sys.argv[1]))["lanes"])))' "$entry")"
[ "$lanes" = "runc,runsc" ] || { echo "FAIL: lanes $lanes (want runc,runsc)"; exit 1; }
[ "$(git -C "$work/check" rev-list --count HEAD)" = 3 ] || { echo "FAIL: history"; exit 1; }
echo "pass: the losing job replayed its lane onto the pushed ledger (lanes $lanes)"
