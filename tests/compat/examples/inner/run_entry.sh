#!/bin/sh
# Runs INSIDE the job Space (never on a host): installs the entry's
# dependencies, runs the unmodified third-party files, prints markers the
# runner parses. Written by us; generated settings come from entry.env.
set -u
# Work on the guest's own disk: venvs, node_modules and caches must not sit on the
# shared job mount (every guest-opened file there stays open on the host).
if [ -z "${EX_RELOCATED:-}" ]; then
    SRC="$(cd "$(dirname "$0")" && pwd)"
    rm -rf /tmp/exjob && mkdir -p /tmp/exjob && cp -R "$SRC"/. /tmp/exjob/ && cd /tmp/exjob \
        && EX_RELOCATED=1 UV_CACHE_DIR=/tmp/uvcache npm_config_cache=/tmp/npmcache exec sh /tmp/exjob/run_entry.sh
fi
JOB="$(cd "$(dirname "$0")" && pwd)"
export JOB
export EX_IN_SPACE=1
cd "$JOB"
. ./entry.env
export PATH="$HOME/.local/bin:$JOB/.venv/bin:$JOB/node_modules/.bin:$PATH"

# uv: the job image ships it; fall back to the official installer only if absent.
if ! command -v uv >/dev/null 2>&1; then
    python3 -m pip install -q --user uv >/dev/null 2>&1 || true
fi

if [ "$EX_LANG" = py ]; then
    if [ -s requirements.txt ]; then
        uv venv -q .venv >/dev/null 2>&1 && uv pip install -q --python .venv/bin/python -r requirements.txt >install.log 2>&1 \
            && echo "@@EX-INSTALL ok" || { echo "@@EX-INSTALL failed: $(tail -3 install.log | tr '\n' ' ')"; echo "@@EX-BEGIN"; echo "@@EX-END exit=97"; exit 0; }
    fi
else
    export NODE_EXTRA_CA_CERTS="${NODE_EXTRA_CA_CERTS:-${SSL_CERT_FILE:-}}"
    if ! command -v node >/dev/null 2>&1; then
        # node via the PyPI wheel of the official binaries, inside this Space only.
        uv venv -q .nodevenv >/dev/null 2>&1 && uv pip install -q --python .nodevenv/bin/python 'nodejs-wheel>=22,<25' >node-install.log 2>&1 \
            && export PATH="$JOB/.nodevenv/bin:$PATH"
    fi
    if [ -f entry.install ]; then
        inst='sh entry.install'
    else
        inst='npm install --no-audit --no-fund --loglevel=error'
    fi
    if command -v node >/dev/null 2>&1 && sh -xc "$inst" >install.log 2>&1; then
        echo "@@EX-INSTALL ok"
    else
        echo "@@EX-INSTALL failed: $( (tail -n 6 install.log; tail -n 3 node-install.log) 2>&1 | tr '\n' ' ') node=$(command -v node)"
        echo "@@EX-BEGIN"; echo "@@EX-END exit=97"; exit 0
    fi
fi

if [ "$EX_MOCK_LLM" = 1 ]; then
    export MOCK_TRACE="$JOB/mock-trace.log"
    python3 "$JOB/mock_openai.py" 8099 >mock.log 2>&1 &
    sleep 1
    export OPENAI_BASE_URL=http://127.0.0.1:8099/v1 OPENAI_API_BASE=http://127.0.0.1:8099/v1 OPENAI_API_KEY=mock
    export ANTHROPIC_BASE_URL=http://127.0.0.1:8099 ANTHROPIC_API_KEY=mock
fi

cd "$JOB/$EX_CWD" 2>/dev/null || cd "$JOB"
echo "@@EX-BEGIN"
if command -v timeout >/dev/null 2>&1; then
    timeout -s KILL "$EX_TIMEOUT" sh -c "$(cat "$JOB/entry.cmd")" 2>&1
else
    sh -c "$(cat "$JOB/entry.cmd")" 2>&1
fi
rc=$?
echo "@@EX-END exit=$rc"
if [ -f "$JOB/junit.xml" ]; then
    base64 < "$JOB/junit.xml" | tr -d '\n' | fold -w 4000 | sed 's/^/@@EX-JUNIT /'
    echo
fi
exit 0
