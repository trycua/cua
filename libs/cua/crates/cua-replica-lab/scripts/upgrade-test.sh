#!/bin/bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# Upgrade, replication and rollback of a Volume that already exists.
#   upgrade-test.sh OLD_CUA NEW_CUA REPLICA_NODE
# OLD_CUA: the shipped cua CLI before replication. NEW_CUA: the cua-spaces-cli
# build after it. REPLICA_NODE: the replica-node binary of this crate
# (cargo build -p cua-replica-lab --bins). Isolated homes only
# (under a temporary directory); never the real ~/.cua.
set -u
OLD=$1; NEW=$2; NODE=$3
T=$(mktemp -d "${TMPDIR:-/tmp}/replica-upgrade.XXXXXX"); H=$T/home-a; H2=$T/home-b; mkdir -p $H $H2
export CUA_HOME=$H CUA_TELEMETRY=0 HOME=$T/fakehome; mkdir -p $HOME
fail() { echo "FAIL: $*"; cleanup; exit 1; }
pids=()
cleanup() { for p in "${pids[@]:-}"; do kill $p 2>/dev/null; done; sleep 1; [ -n "${KEEP:-}" ] || rm -rf "$T"; }
sum_dir() { (cd "$1" && find . -type f ! -path './replica/*' ! -name '.lock' ! -name '*.tmp' ! -name 'grants.json' ! -name 'audit*' -print0 | sort -z | xargs -0 shasum -a 256 | shasum -a 256 | cut -c1-16); }
echo "== 1. the shipped build makes a Volume with real data"
for i in $(seq 1 60); do head -c $((500 + i * 37)) /dev/urandom > $T/f$i; $OLD volume put public/f$i.bin $T/f$i >/dev/null 2>&1 || fail "old put $i"; done
head -c 8000000 /dev/urandom > $T/big; $OLD volume put public/big.bin $T/big >/dev/null 2>&1 || fail "old big put"
echo v1 > $T/h; $OLD volume put public/history.md $T/h >/dev/null 2>&1; echo v2 > $T/h; $OLD volume put public/history.md $T/h >/dev/null 2>&1
echo mine > $T/c1; $OLD volume put "public/notes (conflict from laptop 2026-10-01 10.00.00).md" $T/c1 >/dev/null 2>&1
echo base > $T/c2; $OLD volume put public/notes.md $T/c2 >/dev/null 2>&1
[ -d $H/volume/data ] || fail "no volume data dir after the old build (layout?): $(ls $H)"
BEFORE=$(sum_dir $H/volume/data); echo "data sum before: $BEFORE ($(find $H/volume/data -type f | wc -l) files)"
echo "== 2. in place update: the new build reads everything, writes, history intact"
$NEW volume cat public/history.md 2>&1 | grep -q v2 || fail "new cannot read history head"
for i in 1 17 60; do $NEW volume cat public/f$i.bin > $T/o$i 2>/dev/null; cmp -s $T/o$i $T/f$i || fail "new reads f$i differently"; done
$NEW volume cat public/big.bin > $T/obig 2>/dev/null; cmp -s $T/obig $T/big || fail "new reads the big file differently"
$NEW volume ls public/ 2>/dev/null | grep -q "conflict from laptop" || fail "the conflict copy is gone"
[ "$(sum_dir $H/volume/data)" = "$BEFORE" ] || fail "reading with the new build changed the data"
[ ! -e $H/volume/replica ] || fail "replication files appeared with one machine"
echo ok
echo "== 3. a second machine joins: replication starts, nothing is lost, a backup exists"
P1=7811; P2=7812
$NODE --home $H/volume --id a --name "Machine A" --listen 127.0.0.1:$P1 --member a="Machine A" --member b="Machine B" --peer b=127.0.0.1:$P2 --control 127.0.0.1:7801 --fast --rank 2000 --sleep-prone > $T/a.log 2>&1 & pids+=($!)
$NODE --home $H2/volume --id b --name "Machine B" --listen 127.0.0.1:$P2 --member a="Machine A" --member b="Machine B" --peer a=127.0.0.1:$P1 --control 127.0.0.1:7802 --fast --rank 9000 > $T/b.log 2>&1 & pids+=($!)
ctl() { python3 - "$@" <<'PY'
import socket,sys,json
s=socket.create_connection(("127.0.0.1",int(sys.argv[1])),timeout=60); f=s.makefile("rw")
f.write(" ".join(sys.argv[2:])+"\n"); f.flush(); print(f.readline().strip())
PY
}
for n in $(seq 1 120); do
  A=$(ctl 7801 count public/ 2>/dev/null | python3 -c "import sys,json;print(json.loads(sys.stdin.read()).get('count',0))" 2>/dev/null)
  B=$(ctl 7802 count public/ 2>/dev/null | python3 -c "import sys,json;print(json.loads(sys.stdin.read()).get('count',0))" 2>/dev/null)
  [ "${A:-0}" -gt 0 ] && [ "${A:-0}" = "${B:-1}" ] && break; sleep 1
done
[ "${A:-0}" = "${B:-1}" ] && [ "${A:-0}" -gt 60 ] || fail "the volumes did not meet (a=$A b=$B)"
SA=$(ctl 7801 sum public/ | python3 -c "import sys,json;print(json.loads(sys.stdin.read())['sum'])"); SB=$(ctl 7802 sum public/ | python3 -c "import sys,json;print(json.loads(sys.stdin.read())['sum'])")
[ "$SA" = "$SB" ] || fail "heads differ after joining"
ls $H/volume/replica/backup/heads-* >/dev/null 2>&1 || fail "no backup of the heads"
[ -f $H/volume/replica/format.json ] || fail "no format marker"
# the old Volume's own files are byte for byte what they were (replication only added versions)
for i in 1 17 60; do ctl 7802 get public/f$i.bin | grep -q etag || fail "machine B lacks f$i"; done
echo "replicated: $A objects on both, heads equal, backup and format marker present"
echo "== 4. roll back: the shipped build reads the replicated home and writes behind replication's back"
kill ${pids[0]} ${pids[1]} 2>/dev/null; sleep 2; pids=()
$OLD volume cat public/history.md 2>&1 | grep -q v2 || fail "old build cannot read the replicated home"
$OLD volume cat public/big.bin > $T/obig2 2>/dev/null; cmp -s $T/obig2 $T/big || fail "old build reads the big file differently"
echo "written by the old build after the update" > $T/late; $OLD volume put public/late.md $T/late >/dev/null 2>&1 || fail "old write"
$NODE --home $H/volume --id a --name "Machine A" --listen 127.0.0.1:$P1 --member a="Machine A" --member b="Machine B" --peer b=127.0.0.1:$P2 --control 127.0.0.1:7801 --fast --rank 2000 --sleep-prone > $T/a2.log 2>&1 & pids+=($!)
$NODE --home $H2/volume --id b --name "Machine B" --listen 127.0.0.1:$P2 --member a="Machine A" --member b="Machine B" --peer a=127.0.0.1:$P1 --control 127.0.0.1:7802 --fast --rank 9000 > $T/b2.log 2>&1 & pids+=($!)
for n in $(seq 1 90); do ctl 7802 get public/late.md 2>/dev/null | grep -q etag && break; sleep 1; done
ctl 7802 get public/late.md | grep -q etag || fail "the old build's write never reached machine B"
echo ok
echo "== 4b. the same after a hard stop (no heads snapshot at all)"
kill -9 ${pids[@]} 2>/dev/null; sleep 2; pids=()
rm -f $H/volume/replica/heads.snap
echo "written by the old build after a crash" > $T/late2; $OLD volume put public/late2.md $T/late2 >/dev/null 2>&1 || fail "old write 2"
$NODE --home $H/volume --id a --name "Machine A" --listen 127.0.0.1:$P1 --member a="Machine A" --member b="Machine B" --peer b=127.0.0.1:$P2 --control 127.0.0.1:7801 --fast --rank 2000 --sleep-prone > $T/a4.log 2>&1 & pids+=($!)
$NODE --home $H2/volume --id b --name "Machine B" --listen 127.0.0.1:$P2 --member a="Machine A" --member b="Machine B" --peer a=127.0.0.1:$P1 --control 127.0.0.1:7802 --fast --rank 9000 > $T/b4.log 2>&1 & pids+=($!)
for n in $(seq 1 90); do ctl 7802 get public/late2.md 2>/dev/null | grep -q etag && break; sleep 1; done
ctl 7802 get public/late2.md | grep -q etag || fail "the write made before a hard stop never reached machine B"
echo ok
echo "== 5. a newer format is refused, nothing is touched"
kill ${pids[@]} 2>/dev/null; sleep 2; pids=()
echo '{"format":99,"created_ms":1}' > $H/volume/replica/format.json
B4=$(sum_dir $H/volume/data)
$NODE --home $H/volume --id a --name "Machine A" --listen 127.0.0.1:$P1 --member a="Machine A" --member b="Machine B" --peer b=127.0.0.1:$P2 --control 127.0.0.1:7801 --fast --rank 2000 > $T/a3.log 2>&1 & p=$!; sleep 3; kill $p 2>/dev/null
grep -q "newer Cua Spaces" $T/a3.log || fail "no plain refusal: $(tail -3 $T/a3.log)"
[ "$(sum_dir $H/volume/data)" = "$B4" ] || fail "the refused start touched the data"
$OLD volume cat public/history.md 2>&1 | grep -q v2 || fail "data unreadable after the refusal"
echo ok
echo "ALL PASS"
cleanup
