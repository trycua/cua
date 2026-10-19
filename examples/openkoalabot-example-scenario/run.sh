#!/usr/bin/env bash
# Run the shared openkoalabots scenario (scenario.json) against one lane, for
# one or all implementations, headlessly. Writes one result JSON per run to
# --results and prints a matrix.
#
#   run.sh --impl swift|tauri|ts|all --lane fixture|docker|cloud
#          [--image REF] [--runtime runsc|runc] [--results DIR] [--mock-llm BIN]
#
# Lanes:
#   fixture  `cua-test-fixtures` (libs/cua): a real cua-spacesd server core
#            in-process on loopback, confined to temp HOME/PATH. No desktop,
#            so `stream` skips on the missing `desktop_stream` capability.
#   docker   one linux container per implementation, started
#            with --runtime=runsc (gVisor) --memory=4g and always removed.
#            Implementations run sequentially: one container at a time.
#            A cua-mock-llm container (--memory=256m, always removed) is the
#            scripted model for the routine and group steps' agent turns;
#            --mock-llm is its Linux binary (default: the cargo target's
#            <arch>-unknown-linux-musl/release/cua-mock-llm). Without it
#            those steps skip.
#   cloud    gated: needs OPENKOALABOTS_CLOUD_IMAGE (a linux
#            spacesd image Cua Cloud can pull) and cloud credentials in the
#            environment (CUA_CLIENT_ID/SECRET or FLEETS_TOKEN). The runner
#            creates a Space named openkoalabot-example-scenario-* and deletes it.
#
# Runners never open a window and never touch ~/.cua, ~/.claude, a real app
# profile or the keychain: each uses temp Spaces registries and a generated
# Firefox profile as its teleport home.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SAMPLES="$(cd "$HERE/.." && pwd)"
REPO="$(cd "$SAMPLES/.." && pwd)"
CUA="$REPO/libs/cua"
host_arch() { case "$(uname -m)" in arm64|aarch64) echo arm64 ;; *) echo amd64 ;; esac; }
IMPL=all
LANE=fixture
IMAGE="${OPENKOALABOTS_DOCKER_IMAGE:-cua-e2e-local/linux:docker-local-$(host_arch)}"
RUNTIME=runsc
RESULTS="$HERE/results"
TARGET="${CARGO_TARGET_DIR:-$CUA/target}"
musl_arch() { case "$(uname -m)" in arm64|aarch64) echo aarch64 ;; *) echo x86_64 ;; esac; }
MOCK_LLM="$TARGET/$(musl_arch)-unknown-linux-musl/release/cua-mock-llm"
while [ $# -gt 0 ]; do
    case "$1" in
        --impl) IMPL="$2"; shift 2 ;;
        --lane) LANE="$2"; shift 2 ;;
        --image) IMAGE="$2"; shift 2 ;;
        --runtime) RUNTIME="$2"; shift 2 ;;
        --results) RESULTS="$2"; shift 2 ;;
        --mock-llm) MOCK_LLM="$2"; shift 2 ;;
        -h|--help) sed -n '2,31p' "$0"; exit 0 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
case "$IMPL" in all) IMPLS=(swift tauri ts) ;; swift|tauri|ts) IMPLS=("$IMPL") ;; *) echo "bad --impl $IMPL" >&2; exit 2 ;; esac
mkdir -p "$RESULTS"
SPEC="$HERE/scenario.json"

# The runner command for an implementation (built beforehand; see README).
runner() {
    case "$1" in
        swift) echo "$SAMPLES/openkoalabot-example-swift/.build/debug/OpenKoalaBotExample scenario" ;;
        tauri) echo "${CARGO_TARGET_DIR:-$SAMPLES/openkoalabot-example-tauri/src-tauri/target}/debug/openkoalabot-example-scenario" ;;
        ts) echo "node $SAMPLES/openkoalabot-example-ts/dist/scenario/cli.js" ;;
    esac
}

run_one() { # impl lane -> writes $RESULTS/<impl>-<lane>.json
    local impl="$1" lane="$2" out="$RESULTS/$1-$2.json" start end rc
    rm -f "$out"
    echo "==> $impl on $lane"
    start=$(date +%s)
    set +e
    # shellcheck disable=SC2046
    timeout 900 $(runner "$impl") --spec "$SPEC" --lane "$lane" --out "$out"
    rc=$?
    set -e
    end=$(date +%s)
    if [ ! -s "$out" ]; then
        printf '{"impl":"%s","lane":"%s","ok":false,"totalMs":%d,"steps":[],"error":"runner exited %d without a result"}\n' \
            "$impl" "$lane" $(( (end - start) * 1000 )) "$rc" >"$out"
    fi
}

case "$LANE" in
fixture)
    # build-test-fixtures.sh always builds into libs/cua/target.
    FIX="$TARGET/debug/cua-test-fixtures"
    [ -x "$FIX" ] || FIX="$CUA/target/debug/cua-test-fixtures"
    [ -x "$FIX" ] || CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" "$CUA/scripts/build-test-fixtures.sh"
    for impl in "${IMPLS[@]}"; do
        # One fixture per implementation; it exits when its stdin closes.
        fifo="$(mktemp -u)"; mkfifo "$fifo"
        line_file="$(mktemp)"
        "$FIX" <"$fifo" >"$line_file" 2>/dev/null &
        fix_pid=$!
        exec 9>"$fifo"
        for _ in $(seq 1 100); do [ -s "$line_file" ] && break; sleep 0.1; done
        line="$(head -1 "$line_file")"
        export OPENKOALABOTS_SCENARIO_URL="$(jq -r .spaces_url <<<"$line")"
        export OPENKOALABOTS_SCENARIO_TOKEN="$(jq -r .spaces_token <<<"$line")"
        export OPENKOALABOTS_SCENARIO_IMPORT_ROOT="$(jq -r .spaces_teleport_home <<<"$line")"
        run_one "$impl" fixture || true
        exec 9>&-
        wait "$fix_pid" 2>/dev/null || true
        rm -f "$fifo" "$line_file"
    done
    ;;
docker)
    for impl in "${IMPLS[@]}"; do
        NAME="cua-e2e-openkoalabots-$impl-$$"
        MOCK="cua-e2e-openkoalabots-mock-$impl-$$"
        TOKEN="$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')"
        MOCK_KEY="mock-$(head -c 12 /dev/urandom | od -An -tx1 | tr -d ' \n')"
        ENV_FILE="$(mktemp)"; chmod 600 "$ENV_FILE"
        MOCK_ENV="$(mktemp)"; chmod 600 "$MOCK_ENV"
        printf 'CUA_ENV_TOKEN=%s\n' "$TOKEN" >"$ENV_FILE"
        printf 'CUA_MOCK_LLM_KEY=%s\n' "$MOCK_KEY" >"$MOCK_ENV"
        trap 'docker rm -f "$NAME" "$MOCK" >/dev/null 2>&1 || true; rm -f "$ENV_FILE" "$MOCK_ENV"' EXIT
        unset OPENKOALABOTS_SCENARIO_MODEL_URL ANTHROPIC_API_KEY
        if [ -f "$MOCK_LLM" ]; then
            # The default bridge: gVisor cannot reach Docker's embedded DNS,
            # so the Space reaches the mock by IP.
            docker run -d --name "$MOCK" --memory=256m --env-file "$MOCK_ENV" \
                -v "$MOCK_LLM:/usr/local/bin/cua-mock-llm:ro" debian:bookworm-slim \
                cua-mock-llm --listen 0.0.0.0:8787 >/dev/null
            export OPENKOALABOTS_SCENARIO_MODEL_URL="http://$(docker inspect -f '{{.NetworkSettings.Networks.bridge.IPAddress}}' "$MOCK"):8787"
            export ANTHROPIC_API_KEY="$MOCK_KEY"
        else
            echo "no cua-mock-llm at $MOCK_LLM: the routine and group steps will skip" >&2
        fi
        echo "==> $IMAGE ($RUNTIME) as $NAME"
        docker run -d --name "$NAME" --runtime="$RUNTIME" --memory=4g --memory-swap=4g --shm-size=512m \
            --env-file "$ENV_FILE" -p 127.0.0.1::3211 "$IMAGE" >/dev/null
        status=""
        for _ in $(seq 1 180); do
            status="$(docker inspect -f '{{.State.Health.Status}}' "$NAME" 2>/dev/null || echo gone)"
            { [ "$status" = healthy ] || [ "$status" = gone ]; } && break
            sleep 1
        done
        if [ "$status" != healthy ]; then
            echo "container health: $status" >&2
            printf '{"impl":"%s","lane":"docker","ok":false,"totalMs":0,"steps":[],"error":"container health: %s"}\n' "$impl" "$status" >"$RESULTS/$impl-docker.json"
        else
            PORT="$(docker port "$NAME" 3211/tcp | head -1 | sed 's/.*://')"
            for _ in $(seq 1 60); do
                [ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/health")" = 204 ] && break
                sleep 1
            done
            export OPENKOALABOTS_SCENARIO_URL="http://127.0.0.1:$PORT"
            export OPENKOALABOTS_SCENARIO_TOKEN="$TOKEN"
            export OPENKOALABOTS_SCENARIO_IMPORT_ROOT='$HOME'
            run_one "$impl" docker || true
            docker exec "$NAME" sh -c 'tail -n 200 /var/log/supervisor/cua-spacesd.log' >"$RESULTS/$impl-docker.spacesd.log" 2>&1 || true
            docker logs "$MOCK" >"$RESULTS/$impl-docker.mock-llm.log" 2>&1 || true
        fi
        docker rm -f "$NAME" "$MOCK" >/dev/null 2>&1 || true
        rm -f "$ENV_FILE" "$MOCK_ENV"
        unset OPENKOALABOTS_SCENARIO_MODEL_URL ANTHROPIC_API_KEY
        trap - EXIT
    done
    ;;
cloud)
    if [ -z "${OPENKOALABOTS_CLOUD_IMAGE:-}" ]; then
        echo "cloud lane skipped: OPENKOALABOTS_CLOUD_IMAGE is unset (no linux spacesd image in a registry Cua Cloud can pull)"
        for impl in "${IMPLS[@]}"; do
            printf '{"impl":"%s","lane":"cloud","ok":true,"skipped":true,"totalMs":0,"steps":[],"error":"OPENKOALABOTS_CLOUD_IMAGE is unset"}\n' "$impl" >"$RESULTS/$impl-cloud.json"
        done
    else
        unset OPENKOALABOTS_SCENARIO_URL OPENKOALABOTS_SCENARIO_TOKEN
        export OPENKOALABOTS_SCENARIO_IMPORT_ROOT='$HOME'
        for impl in "${IMPLS[@]}"; do run_one "$impl" cloud || true; done
    fi
    ;;
*) echo "bad --lane $LANE" >&2; exit 2 ;;
esac

# Matrix: impl x step.
echo
printf '| impl | lane | result | total | %s |\n' "$(jq -r '[.steps[].id] | join(" | ")' "$SPEC")"
printf '|---|---|---|---|%s\n' "$(jq -r '[.steps[] | "---|"] | join("")' "$SPEC")"
fail=0
for impl in "${IMPLS[@]}"; do
    f="$RESULTS/$impl-$LANE.json"
    jq -r --slurpfile spec "$SPEC" '
      . as $r
      | ($spec[0].steps | map(.id)) as $ids
      | [ $r.impl, $r.lane,
          (if $r.skipped then "skipped" elif $r.ok then "pass" else "FAIL" end),
          "\(($r.totalMs // 0) / 1000 | . * 10 | round / 10)s" ]
        + [ $ids[] as $id | ($r.steps | map(select(.id == $id)) | first) as $s
            | if $s == null then "-" else "\($s.status) \(($s.ms // 0) / 1000 | . * 10 | round / 10)s" end ]
      | "| " + join(" | ") + " |"' "$f"
    [ "$(jq -r '.ok' "$f")" = true ] || fail=1
done
for impl in "${IMPLS[@]}"; do
    jq -r '.steps[]? | select(.status == "fail") | "  \(input_filename): \(.id): \(.detail)"' "$RESULTS/$impl-$LANE.json" 2>/dev/null || true
    jq -r 'select(.error) | "  \(.impl): \(.error)"' "$RESULTS/$impl-$LANE.json"
done
exit "$fail"
