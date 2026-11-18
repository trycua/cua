#!/usr/bin/env bash
# shellcheck disable=SC2015 # `cond && ok || bad`: ok never fails
# Offline tests for content-digest.sh and report-schedule-failure.sh (fake
# crane and gh; no network, no registry, no issues touched).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
pass=0 fail=0
ok() { pass=$((pass+1)); }
bad() { fail=$((fail+1)); echo "FAIL: $*"; }

# --- content-digest: package order does not matter; content and trees do ----
printf 'b=2\na=1\n' >"$tmp/p1"; printf 'a=1\nb=2\n' >"$tmp/p2"; printf 'a=1\nb=3\n' >"$tmp/p3"
d1="$("$HERE/content-digest.sh" --packages "$tmp/p1")"
d2="$("$HERE/content-digest.sh" --packages "$tmp/p2")"
d3="$("$HERE/content-digest.sh" --packages "$tmp/p3")"
[[ "$d1" =~ ^sha256:[0-9a-f]{64}$ ]] && ok || bad "digest format $d1"
[ "$d1" = "$d2" ] && ok || bad "order changed the digest"
[ "$d1" != "$d3" ] && ok || bad "a version bump kept the digest"
t1="$(cd "$HERE/../.." && "$HERE/content-digest.sh" --packages "$tmp/p1" --tree scripts/images)"
[ "$t1" != "$d1" ] && ok || bad "a tree did not change the digest"
(cd "$HERE/../.." && "$HERE/content-digest.sh" --packages "$tmp/p1" --tree scripts/images --out "$tmp/why" >/dev/null)
grep -q '^tree scripts/images ' "$tmp/why" && grep -q '^# digest sha256:' "$tmp/why" && ok || bad "--out evidence"
if (cd "$HERE/../.." && "$HERE/content-digest.sh" --packages "$tmp/p1" --tree no/such/dir) >/dev/null 2>&1; then
    bad "missing tree accepted"; else ok; fi

# --promoted reads the annotation; a missing ref or annotation prints nothing.
cat >"$tmp/crane" <<'FAKE'
#!/usr/bin/env bash
case "$2" in
  *:24.04) echo '{"annotations":{"ai.cua.image.content-digest":"sha256:abc"}}' ;;
  *:old) echo '{"annotations":{}}' ;;
  *) echo "MANIFEST_UNKNOWN" >&2; exit 1 ;;
esac
FAKE
chmod +x "$tmp/crane"
[ "$(CRANE="$tmp/crane" "$HERE/content-digest.sh" --promoted r:24.04)" = sha256:abc ] && ok || bad "--promoted annotation"
[ -z "$(CRANE="$tmp/crane" "$HERE/content-digest.sh" --promoted r:old)" ] && ok || bad "--promoted no annotation"
[ -z "$(CRANE="$tmp/crane" "$HERE/content-digest.sh" --promoted r:none)" ] && ok || bad "--promoted missing ref"

# --- report-schedule-failure: comment on the open issue, else open one --------
mkdir -p "$tmp/bin"
cat >"$tmp/bin/gh" <<'FAKE'
#!/usr/bin/env bash
echo "$*" >>"$GH_LOG"
if [ "$1 $2" = "issue list" ]; then [ -n "${GH_OPEN:-}" ] && echo "$GH_OPEN"; fi
exit 0
FAKE
chmod +x "$tmp/bin/gh"
export GH_LOG="$tmp/gh.log"
: >"$GH_LOG"
out="$(PATH="$tmp/bin:$PATH" GH_OPEN=42 "$HERE/report-schedule-failure.sh" "CD: Image linux" https://x/run/1)"
[ "$out" = "commented on #42" ] && grep -q '^issue comment 42 ' "$GH_LOG" && ok || bad "comment path: $out"
: >"$GH_LOG"
out="$(PATH="$tmp/bin:$PATH" "$HERE/report-schedule-failure.sh" "CD: Image linux" https://x/run/2 "doctor failed")"
[ "$out" = "opened an issue" ] && grep -q '^issue create --title Scheduled image rebuild failing: CD: Image linux --label image-schedule' "$GH_LOG" && ok ||
    bad "create path: $out $(cat "$GH_LOG")"
grep -q '^issue comment' "$GH_LOG" && bad "commented when none was open" || ok

echo "schedule scripts: $pass passed, $fail failed"
[ "$fail" = 0 ]
