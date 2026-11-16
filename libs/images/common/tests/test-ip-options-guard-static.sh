#!/usr/bin/env bash
# Static checks for the CVE-2026-98432 interim guard: the files exist, every
# Linux VM recipe installs and enables them, the ruleset stays in its own table,
# and the CVE watch (common/vm/cve-kernel-watch.json) agrees with the kernels the
# images' evidence recorded. No Docker, no network.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMAGES="$(cd "$HERE/../.." && pwd)"
G="$IMAGES/common/vm/files/cua-ipopts"
pass=0 fail=0
ok()  { pass=$((pass + 1)); }
bad() { fail=$((fail + 1)); echo "FAIL $*"; }
has() { # name file pattern (fixed string)
    if grep -qF -- "$3" "$2"; then ok; else bad "$1: '$3' missing from ${2#"$IMAGES"/}"; fi
}

for f in cua-ipopts.nft cua-ipopts-guard cua-ipopts.service 90-cua-ipopts.conf; do
    [ -s "$G/$f" ] && ok || bad "missing $f"
done
[ -x "$G/cua-ipopts-guard" ] && ok || bad "cua-ipopts-guard is not executable"

# ruleset: own table, early hook, header-length drop, never a global flush
has ruleset "$G/cua-ipopts.nft" "table ip cua_ipopts"
has ruleset "$G/cua-ipopts.nft" "hook prerouting priority -450"
has ruleset "$G/cua-ipopts.nft" "ip hdrlength > 5"
has ruleset "$G/cua-ipopts.nft" "delete table ip cua_ipopts"
if grep -qE '^[[:space:]]*flush[[:space:]]+ruleset' "$G/cua-ipopts.nft"; then bad "ruleset flushes other tables"; else ok; fi
if grep -nE '^[[:space:]]*(delete|flush)[[:space:]]+(table|chain)' "$G/cua-ipopts.nft" | grep -v cua_ipopts >/dev/null; then bad "ruleset touches another table"; else ok; fi

# unit: before the network, runs the guard, enabled at sysinit
has unit "$G/cua-ipopts.service" "Before=network-pre.target"
has unit "$G/cua-ipopts.service" "ExecStart=/opt/cua/bin/cua-ipopts-guard start"
has unit "$G/cua-ipopts.service" "WantedBy=sysinit.target"
has sysctl "$G/90-cua-ipopts.conf" "net.ipv4.conf.all.accept_source_route = 0"

# every VM recipe installs it
for df in common/vm/Dockerfile omarchy/vm.Dockerfile; do
    for f in "cua-ipopts-guard /opt/cua/bin/cua-ipopts-guard" \
             "cua-ipopts.nft /etc/cua-image/cua-ipopts.nft" \
             "cua-ipopts.service /etc/systemd/system/cua-ipopts.service" \
             "90-cua-ipopts.conf /etc/sysctl.d/90-cua-ipopts.conf"; do
        has "$df" "$IMAGES/$df" "common/vm/files/cua-ipopts/$f"
    done
    has "$df" "$IMAGES/$df" "cua-ipopts.service"
done
has "common/vm/Dockerfile enable" "$IMAGES/common/vm/Dockerfile" "qemu-guest-agent cua-ipopts.service"
has "common/vm/Dockerfile nft" "$IMAGES/common/vm/Dockerfile" "iproute2 nftables"
has "omarchy nft" "$IMAGES/omarchy/vm.Dockerfile" "dosfstools nftables"
has "omarchy enable" "$IMAGES/omarchy/vm.Dockerfile" "systemctl enable cua-ipopts.service"
has "osworld upload" "$IMAGES/bench/osworld/vm/customize.sh" 'upload $COMMON_VM/cua-ipopts.nft /etc/cua-image/cua-ipopts.nft'
has "osworld unit" "$IMAGES/bench/osworld/vm/customize.sh" 'ln-sf /etc/systemd/system/cua-ipopts.service /etc/systemd/system/sysinit.target.wants/cua-ipopts.service'
has "osworld verify" "$IMAGES/bench/osworld/vm/customize.sh" 'is-symlink /etc/systemd/system/sysinit.target.wants/cua-ipopts.service'

# the runtime test's throwaway image installs it with the same lines as the recipe
while IFS= read -r line; do
    grep -qxF -- "$line" "$IMAGES/common/tests/ip-options-guard.Dockerfile" && ok || bad "runtime-test Dockerfile drifted: $line"
done < <(grep -E '^COPY common/vm/files/cua-ipopts/' "$IMAGES/common/vm/Dockerfile")

# CVE watch
python3 - "$IMAGES" <<'PY' || fail=$((fail + 1))
import json, os, re, sys
images = sys.argv[1]
w = json.load(open(f"{images}/common/vm/cve-kernel-watch.json"))
def key(v):
    return tuple(int(x) for x in re.findall(r"\d+", v.split("-generic")[0].split("-virtual")[0]))
bad = False
for img, ev in w["images"].items():
    path = f"{images}/{img}/evidence/{ev}"
    doc = json.load(open(path))
    kernels = set()
    def walk(o):
        if isinstance(o, dict):
            for k, v in o.items():
                if k == "kernel" and isinstance(v, str): kernels.add(v)
                else: walk(v)
        elif isinstance(o, list):
            for v in o: walk(v)
    walk(doc)
    for k in sorted(kernels):
        series = ".".join(map(str, key(k)[:2]))
        s = w["series"].get(series)
        if s is None:
            print(f"FAIL {img}: kernel {k} is in series {series}, which cve-kernel-watch.json does not list"); bad = True; continue
        fixed = s["fixed"]
        if fixed and key(k) >= key(fixed):
            print(f"NOTE {img}: kernel {k} is at or above {fixed}; the guard can be removed from this image")
        elif s["published"]:
            print(f"FAIL {img}: kernel {k} is older than the published fix {fixed}; rebuild and republish the image"); bad = True
        else:
            print(f"watch {img}: kernel {k} < {fixed or 'no fix'} (fix not published yet); covered by the guard")
sys.exit(1 if bad else 0)
PY
echo "static: pass=$pass fail=$fail"
[ "$fail" = 0 ]
