#!/usr/bin/env bash
# Run `cua-spacesd doctor` against one image variant in one lane and collect
# the report. The single implementation behind the image CI gates: the
# linux build job runs it before pushing, and
# .github/workflows/image-doctor.yml runs it for published refs.
#
#   image-doctor-lane.sh --lane runc|runsc --image REF [--out DIR] [options]
#   image-doctor-lane.sh --lane qemu --disk DISK.img [--arch amd64|arm64] [--claim-secrets] [--out DIR] [options]
#
# Lanes:
#   runc    docker run the rootfs (4g memory cap, 512m shm), docker exec the
#           doctor as root
#   runsc   the same under gVisor (--runtime=runsc; native arch only)
#   qemu    boot the containerDisk payload (libs/images/common/boot-qemu.sh,
#           4G RAM, KVM/HVF when usable), ssh in, run the doctor as root.
#           --claim-secrets delivers the token through the claim-secrets
#           share instead of a local file (the KubeVirt-shaped auth path:
#           virtiofs on Linux, 9p on macOS)
#
# Options:
#   --strict (default on) / --no-strict
#   --effects virtual|none   default virtual (fixture windows only, inside
#                            the sandbox's own virtual display)
#   --expect-manifest FILE   check against this manifest instead of the
#                            image's own
#   --only / --skip LIST     passed through
#   --timeout SECS           doctor budget (default 300; 3600 under TCG)
#   --timeout-scale N        CUA_DOCTOR_TIMEOUT_SCALE for the doctor (1-20;
#                            default: the environment's, else 8 for a qemu
#                            lane that boots under TCG, else 1)
#   --boot-timeout SECS      qemu: boot budget (default 1200; 3600 under TCG)
#   --doctor-bin FILE        qemu: run this cua-spacesd build's doctor against
#                            the image's running service instead of the
#                            image's own binary (doctoring already-published
#                            disks with a newer doctor; the service, the
#                            image and its manifest are unchanged)
#
# Output in DIR (default ./doctor-out/<lane>): report.json, report.xml
# (JUnit), report.txt, lane.json ({lane, image, arch, variant, exit}), and
# container/serial logs. Exit status: the doctor's (0 pass, 1 fail, 2 the
# doctor or the lane could not run).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
host_arch() { case "$(uname -m)" in arm64|aarch64) echo arm64 ;; *) echo amd64 ;; esac; }

LANE="" IMAGE="" DISK="" ARCH="$(host_arch)" OUT="" STRICT=1 EFFECTS=virtual CLAIM=0
EXPECT="" ONLY="" SKIP="" TIMEOUT="" SCALE="${CUA_DOCTOR_TIMEOUT_SCALE:-}" BOOT_TIMEOUT="" DOCTOR_BIN=""
while [ $# -gt 0 ]; do
    case "$1" in
        --lane) LANE="$2"; shift 2 ;;
        --image) IMAGE="$2"; shift 2 ;;
        --disk) DISK="$2"; shift 2 ;;
        --arch) ARCH="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        --strict) STRICT=1; shift ;;
        --no-strict) STRICT=0; shift ;;
        --effects) EFFECTS="$2"; shift 2 ;;
        --claim-secrets) CLAIM=1; shift ;;
        --expect-manifest) EXPECT="$2"; shift 2 ;;
        --only) ONLY="$2"; shift 2 ;;
        --skip) SKIP="$2"; shift 2 ;;
        --timeout) TIMEOUT="$2"; shift 2 ;;
        --timeout-scale) SCALE="$2"; shift 2 ;;
        --boot-timeout) BOOT_TIMEOUT="$2"; shift 2 ;;
        --doctor-bin) DOCTOR_BIN="$2"; shift 2 ;;
        -h|--help) sed -n '2,33p' "$0"; exit 0 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
case "$LANE" in
    runc|runsc) [ -n "$IMAGE" ] || { echo "--lane $LANE needs --image" >&2; exit 2; } ;;
    qemu) [ -f "$DISK" ] || { echo "--lane qemu needs --disk <disk.img>" >&2; exit 2; }
          [ -z "$DOCTOR_BIN" ] || [ -f "$DOCTOR_BIN" ] || { echo "--doctor-bin $DOCTOR_BIN does not exist" >&2; exit 2; } ;;
    *) echo "--lane must be runc, runsc or qemu" >&2; exit 2 ;;
esac
# A qemu lane boots under TCG when boot-qemu.sh would (no KVM/HVF for the
# guest arch, or BOOT_QEMU_ACCEL=tcg): every step runs several times slower,
# so budgets stretch. Hosted arm64 runners are the TCG case.
TCG=0
if [ "$LANE" = qemu ]; then
    host_arch="$(uname -m)"; case "$host_arch" in x86_64) host_arch=amd64 ;; aarch64|arm64) host_arch=arm64 ;; esac
    if [ "${BOOT_QEMU_ACCEL:-}" = tcg ]; then TCG=1
    elif [ -z "${BOOT_QEMU_ACCEL:-}" ] && { [ "${ARCH:-$host_arch}" != "$host_arch" ] || { [ "$(uname -s)" != Darwin ] && [ ! -w /dev/kvm ]; }; }; then TCG=1; fi
fi
if [ "$TCG" = 1 ]; then
    TIMEOUT="${TIMEOUT:-3600}" SCALE="${SCALE:-8}" BOOT_TIMEOUT="${BOOT_TIMEOUT:-3600}"
fi
TIMEOUT="${TIMEOUT:-300}" SCALE="${SCALE:-1}" BOOT_TIMEOUT="${BOOT_TIMEOUT:-1200}"
[ -z "$DOCTOR_BIN" ] || DOCTOR_BIN="$(cd "$(dirname "$DOCTOR_BIN")" && pwd)/$(basename "$DOCTOR_BIN")"
OUT="${OUT:-$PWD/doctor-out/$LANE}"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"

args=(doctor --format human --progress --effects "$EFFECTS" --timeout "$TIMEOUT"
      --out /tmp/cua-doctor/report.json --junit /tmp/cua-doctor/report.xml --artifacts /tmp/cua-doctor/artifacts)
case "$LANE" in runc) args+=(--expect-runtime container) ;; runsc) args+=(--expect-runtime gvisor) ;; qemu) args+=(--expect-runtime qemu) ;; esac
[ "$STRICT" = 1 ] && args+=(--strict)
[ -n "$ONLY" ] && args+=(--only "$ONLY")
[ -n "$SKIP" ] && args+=(--skip "$SKIP")
# A token the lane controls (never printed): the doctor finds it in the
# container environment / the VM's token file like any client would.
TOKEN="cua-e2e-doctor-$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')"

write_lane() {
    python3 - "$OUT/lane.json" "$LANE" "${IMAGE:-$DISK}" "$ARCH" "$1" "$CLAIM" <<'PY'
import json, sys
path, lane, image, arch, code, claim = sys.argv[1:]
variant = "containerdisk" if lane == "qemu" else "rootfs"
json.dump({"lane": lane, "image": image, "arch": arch, "variant": variant,
           "claim_secrets": claim == "1", "exit": int(code)}, open(path, "w"), indent=2)
PY
}

summarize() {
    if [ -s "$OUT/report.json" ]; then
        python3 - "$OUT/report.json" <<'PY'
import json, sys
r = json.load(open(sys.argv[1]))
s = r["summary"]
print(f"doctor {s['status']}: {s['pass']} pass, {s['warn']} warn, {s['fail']} fail, {s['skip']} skip")
for c in r["checks"]:
    if c["status"] == "fail" or (s.get("strict") and c["status"] == "warn"):
        print(f"  {c['status']:4} {c['id']}: {c['message']}")
PY
    else
        echo "no report.json was produced"
    fi
}

NAME=""
container_lane() {
    local runtime="$1"
    local manifest_args=()
    NAME="cua-e2e-doctor-$LANE-$$"
    cleanup() {
        docker logs "$NAME" >"$OUT/container.log" 2>&1 || true
        docker exec "$NAME" sh -c 'tail -n 300 /var/log/supervisor/*.log' >"$OUT/guest-logs.txt" 2>&1 || true
        docker rm -f "$NAME" >/dev/null 2>&1 || true
    }
    trap cleanup EXIT
    echo "==> $IMAGE under --runtime=$runtime"
    # Run it the way cua does: under gVisor with SYS_ADMIN, which stays in
    # gVisor's own kernel and lets cua-spacesd mount Cua Volume (FUSE); runc
    # sandboxes get no extra capability (the volume is unsupported there).
    local caps=()
    [ "$runtime" = runsc ] && caps=(--cap-add SYS_ADMIN)
    docker run -d --name "$NAME" --runtime="$runtime" --shm-size=512m \
        ${caps[@]+"${caps[@]}"} \
        --memory=4g --memory-swap=4g -e CUA_ENV_TOKEN="$TOKEN" "$IMAGE" >/dev/null
    local status=""
    for _ in $(seq 1 120); do
        status="$(docker inspect -f '{{.State.Health.Status}}' "$NAME" 2>/dev/null || echo gone)"
        [ "$status" = healthy ] || [ "$status" = gone ] && break
        sleep 1
    done
    if [ "$status" != healthy ]; then
        echo "container never became healthy (status: $status)" >&2
        return 2
    fi
    # The service answers /health once it serves.
    for _ in $(seq 1 60); do
        docker exec "$NAME" sh -c 'exec 3<>/dev/tcp/127.0.0.1/3211' 2>/dev/null && break
        sleep 1
    done
    if [ -n "$EXPECT" ]; then
        docker cp "$EXPECT" "$NAME:/tmp/cua-doctor-expect.json"
        manifest_args=(--expect-manifest /tmp/cua-doctor-expect.json)
    fi
    docker exec "$NAME" mkdir -p /tmp/cua-doctor
    set +e
    docker exec "$NAME" cua-spacesd "${args[@]}" "${manifest_args[@]+"${manifest_args[@]}"}" \
        --target http://127.0.0.1:3211 | tee "$OUT/report.txt"
    local rc=${PIPESTATUS[0]}
    set -e
    # Through exec, not `docker cp`: gVisor keeps /tmp inside the sandbox.
    docker exec "$NAME" tar -C /tmp/cua-doctor -cf - . | tar -C "$OUT" -xf - 2>/dev/null || true
    return "$rc"
}

qemu_lane() {
    # shellcheck disable=SC2054  # "3211,22" is one comma-separated port list
    local claim_dir="" boot_args=(--arch "$ARCH" --mem 4G --smp 4 --fwd 3211,22 --ssh --timeout "$BOOT_TIMEOUT" --log "$OUT/serial.log")
    local work
    work="$(mktemp -d "${TMPDIR:-/tmp}/cua-e2e-doctor.XXXXXX")"
    if [ "$CLAIM" = 1 ]; then
        claim_dir="$work/claim"
        mkdir -p "$claim_dir"
        printf '%s' "$TOKEN" >"$claim_dir/env-token"
        chmod 0600 "$claim_dir/env-token"
        boot_args+=(--claim-secrets "$claim_dir")
    else
        boot_args+=(--env-token "$TOKEN")
    fi
    local expect_copy=""
    if [ -n "$EXPECT" ]; then expect_copy="$EXPECT"; fi
    # Runs on the host once the VM is up: wait for the service, run the
    # doctor as root over ssh, copy the report back.
    cat >"$work/run.sh" <<EOF
set -uo pipefail
ssh_() { ssh -i "\$QEMU_SSH_KEY" -p "\$QEMU_FWD_22" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o BatchMode=yes -o ConnectTimeout=10 cua@127.0.0.1 "\$@"; }
# The guest is settled before the doctor starts: cloud-init's runcmd
# restarts cua-spacesd once it has written the token (boot-qemu.sh), and
# under TCG that lands minutes after sshd, in the middle of a doctor run.
doctor=cua-spacesd
if [ -n "$DOCTOR_BIN" ]; then
    ssh_ 'cat >/tmp/cua-doctor-bin && chmod 0755 /tmp/cua-doctor-bin' <"$DOCTOR_BIN"
    doctor=/tmp/cua-doctor-bin
fi
ssh_ 'sudo timeout $((600 * ${SCALE%.*})) cloud-init status --wait >/dev/null 2>&1 || true'
for _ in \$(seq 1 $((90 * ${SCALE%.*}))); do ssh_ 'curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:3211/health' 2>/dev/null | grep -q 204 && break; sleep 2; done
# Then every Health component serves (the desktop session up), or the
# budget is spent and meta.health reports what is not.
for _ in \$(seq 1 $((60 * ${SCALE%.*}))); do
    ssh_ "sudo \$doctor doctor --only meta.health --timeout 30 --target http://127.0.0.1:3211 >/dev/null 2>&1" && break
    sleep 5
done
# A just-booted guest may not have synced its clock yet, and time.sync warns
# after its own 30 s: give systemd-timesyncd up to 90 s more first (bounded;
# a guest that never syncs still gets the doctor's warning).
if ssh_ 'command -v timedatectl' >/dev/null 2>&1; then
    for _ in \$(seq 1 45); do
        ssh_ 'timedatectl show -p NTPSynchronized --value' 2>/dev/null | grep -qx yes && break
        sleep 2
    done
fi
extra=""
if [ -n "$expect_copy" ]; then
    ssh_ 'cat >/tmp/cua-doctor-expect.json' <"$expect_copy"
    extra="--expect-manifest /tmp/cua-doctor-expect.json"
fi
ssh_ "sudo mkdir -p /tmp/cua-doctor && sudo env CUA_DOCTOR_TIMEOUT_SCALE=$SCALE \$doctor ${args[*]} \$extra --target http://127.0.0.1:3211" | tee "$OUT/report.txt"
rc=\${PIPESTATUS[0]}
ssh_ 'sudo tar -C /tmp/cua-doctor -cf - .' | tar -C "$OUT" -xf - 2>/dev/null || true
ssh_ 'sudo journalctl --no-pager -u "cua-*" -n 400' >"$OUT/guest-journal.txt" 2>&1 || true
exit \$rc
EOF
    set +e
    "$REPO/libs/images/common/boot-qemu.sh" "$DISK" "${boot_args[@]}" --run "bash $work/run.sh" | tee "$OUT/boot.txt"
    local rc=${PIPESTATUS[0]}
    set -e
    rm -rf "$work"
    return "$rc"
}

set +e
case "$LANE" in
    runc) (container_lane runc) ;;
    runsc) (container_lane runsc) ;;
    qemu) (qemu_lane) ;;
esac
rc=$?
set -e
# boot-qemu.sh folds every failure into 1; trust the report when present.
if [ -s "$OUT/report.json" ]; then
    status="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["summary"]["status"])' "$OUT/report.json")"
    if [ "$status" = fail ] || { [ "$STRICT" = 1 ] && [ "$status" = warn ]; }; then rc=1; elif [ "$rc" != 2 ]; then rc=0; fi
elif [ "$rc" = 0 ]; then
    rc=2
fi
write_lane "$rc"
summarize
echo "==> lane $LANE: exit $rc, report in $OUT"
exit "$rc"
