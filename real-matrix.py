#!/usr/bin/env python3
"""Real-harness connection matrix (CUA-2268), in a throwaway HOME.

Installs each agent for real (npm / official installers) into a temporary
HOME, walks it through the matrix states, and records what `cua connect
status --json` and the `connect_status` MCP tool say at each step (they must
agree). No real sign-in: signed-in cells use what the harness itself writes
for an API key. Prints a JSON report on stdout. Temporary file: not
committed.
"""
import json, os, re, shutil, subprocess, sys, tempfile, time

CUA = os.path.abspath(sys.argv[1])
ROOT = tempfile.mkdtemp(prefix="rm-")
HOME = os.path.join(ROOT, "home")
os.makedirs(os.path.join(HOME, "bin"))
os.symlink(CUA, os.path.join(HOME, "bin", "cua"))
NPM1 = os.path.join(HOME, ".npm-global")
NPM2 = os.path.join(HOME, ".npm-second")
BASE_PATH = f"{HOME}/bin:{NPM1}/bin:{HOME}/.local/bin:/usr/local/bin:/usr/bin:/bin"
ENV = dict(
    HOME=HOME, CUA_HOME=f"{HOME}/.cua", XDG_CONFIG_HOME=f"{HOME}/.config",
    CUA_AGENT_APP_DIRS="", CUA_AGENT_BIN_DIRS="", CUA_TEST="1", CUA_TELEMETRY="0",
    CUA_TELEMETRY_FORBID_NETWORK="1", CUA_CREDENTIAL_STORE=f"test-keychain:{ROOT}/kc",
    CUA_LOG="error", PATH=BASE_PATH, LANG="C.UTF-8", TERM="dumb",
)
os.makedirs(f"{ROOT}/kc")
LOG = []


def sh(cmd, env=None, timeout=600, path=None):
    e = dict(ENV, **(env or {}))
    if path:
        e["PATH"] = path
    p = subprocess.run(cmd, shell=True, env=e, cwd=HOME, capture_output=True, text=True, timeout=timeout)
    LOG.append({"cmd": cmd, "code": p.returncode, "out": p.stdout[-400:], "err": p.stderr[-400:]})
    return p


def mcp_status(path=None):
    e = dict(ENV)
    if path:
        e["PATH"] = path
    p = subprocess.Popen(["cua", "--embedded", "mcp"], env=e, cwd=HOME, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    def send(i, m, params):
        p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": i, "method": m, "params": params}) + "\n"); p.stdin.flush()
        while True:
            line = p.stdout.readline()
            if not line:
                raise RuntimeError("mcp closed")
            v = json.loads(line)
            if v.get("id") == i:
                return v
    try:
        send(1, "initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "matrix", "version": "0"}})
        v = send(2, "tools/call", {"name": "connect_status", "arguments": {}})
        return json.loads(v["result"]["content"][0]["text"])
    finally:
        p.kill(); p.wait()


def strip(r):
    r = json.loads(json.dumps(r))
    r.pop("checked_at_ms", None)
    for a in r["agents"]:
        a.pop("checked_at_ms", None)
    return r["agents"]


RESULTS = []


def status(harness, cell, path=None):
    p = sh("cua connect status --json", path=path)
    try:
        cli = json.loads(p.stdout)
    except Exception:
        RESULTS.append({"harness": harness, "cell": cell, "error": p.stderr[-300:]}); return None
    try:
        mcp = mcp_status(path)
        agree = strip(cli) == strip(mcp)
    except Exception as e:
        agree = f"mcp failed: {e}"
    row = next((a for a in cli["agents"] if a["id"] == harness), None)
    RESULTS.append({"harness": harness, "cell": cell, "cli_mcp_agree": agree, "row": {k: row[k] for k in ("state", "installed", "version", "previous_version", "signed_in", "mcp", "signin_import", "problem")} if row else None})
    return row


def rename_entry(cfg):
    """Removes Cua's entry by renaming it, as a person editing the file would."""
    t = open(cfg).read()
    t2 = re.sub(r'\[mcp_servers\.cua\]', '[mcp_servers.mine]', t)
    t2 = re.sub(r'"cua"(\s*:)', r'"mine"\1', t2)
    t2 = re.sub(r'(?m)^(\s+)cua:', r'\1mine:', t2)
    t2 = re.sub(r'(?m)^(\s+)cua(\s*:\s*\{)', r'\1mine\2', t2)
    open(cfg, "w").write(t2)


def break_entry(cfg):
    t = open(cfg).read()
    open(cfg, "w").write(t.replace(os.path.join(HOME, "bin", "cua"), "/nowhere/cua").replace(os.path.realpath(CUA), "/nowhere/cua"))


def versions(pkg):
    p = sh(f"npm view {pkg} versions --json", timeout=120)
    try:
        v = [x for x in json.loads(p.stdout) if re.fullmatch(r"[0-9.]+", x)]
        return v[-2], v[-1]
    except Exception:
        return None, None


HARNESSES = [
    # id, npm package or None, binary, config file, how to sign in without a person
    ("codex", "@openai/codex", "codex", ".codex/config.toml",
     "printf %s sk-test-not-a-real-key | codex login --with-api-key"),
    ("claude-code", "@anthropic-ai/claude-code", "claude", ".claude.json", None),
    ("openclaw", "openclaw", "openclaw", ".openclaw/openclaw.json", None),
    ("t3code", "t3", "t3", ".codex/config.toml", None),
]


def run_npm_harness(hid, pkg, binary, cfg_rel, signin):
    cfg = os.path.join(HOME, cfg_rel)
    status(hid, "not installed")
    old, new = versions(pkg)
    if not new:
        RESULTS.append({"harness": hid, "cell": "install", "error": "npm view failed"}); return
    p = sh(f"npm install -g --prefix {NPM1} {pkg}@{old}", timeout=900)
    if p.returncode != 0:
        RESULTS.append({"harness": hid, "cell": "install", "error": p.stderr[-300:]}); return
    status(hid, f"installed {old}, not signed in, not connected")
    sh(f"cua connect setup --agents {hid} -y --json")
    status(hid, "connected (tools), not signed in")
    if signin:
        sh(signin)
        status(hid, "signed in (API key), connected")
        sh(f"cua connect setup --agents {hid} --sign-in -y --json")
        status(hid, "signed in, sign-in copied to Keyvault")
    if os.path.exists(cfg):
        break_entry(cfg)
        status(hid, "config edited outside Cua (cua path gone)")
        sh(f"cua connect setup --agents {hid} -y --json")
        status(hid, "after Reconnect")
        rename_entry(cfg)
        status(hid, "config entry removed outside Cua")
        sh(f"cua connect setup --agents {hid} -y --json")
        status(hid, "after Reconnect (2)")
    sh(f"npm install -g --prefix {NPM1} {pkg}@{new}", timeout=900)
    status(hid, f"harness upgraded {old} -> {new}")
    sh(f"npm install -g --prefix {NPM2} {pkg}@{old}", timeout=900)
    status(hid, "a second install of the same harness first on PATH", path=f"{HOME}/bin:{NPM2}/bin:{NPM1}/bin:/usr/local/bin:/usr/bin:/bin")
    sh(f"npm uninstall -g --prefix {NPM1} {pkg}", timeout=300)
    shutil.rmtree(NPM2, ignore_errors=True)
    status(hid, "uninstalled after connecting")
    sh(f"cua connect remove --agents {hid} --sign-in -y --json")
    status(hid, "after Disconnect")


def main():
    sh("node --version; npm --version")
    for h in HARNESSES:
        try:
            run_npm_harness(*h)
        except Exception as e:
            RESULTS.append({"harness": h[0], "error": repr(e)})
    # Hermes: its installer (uv-managed Python) into the throwaway HOME.
    try:
        status("hermes", "not installed")
        p = sh("curl -fsSL https://raw.githubusercontent.com/NousResearch/hermes-agent/main/scripts/install.sh | bash -s -- --skip-setup", timeout=1500)
        hermes = shutil.which("hermes", path=ENV["PATH"]) or next((os.path.join(d, "hermes") for d in [f"{HOME}/.local/bin", f"{HOME}/.hermes/bin"] if os.path.exists(os.path.join(d, "hermes"))), None)
        if hermes:
            os.makedirs(f"{HOME}/bin", exist_ok=True)
            if not os.path.exists(f"{HOME}/bin/hermes"):
                os.symlink(hermes, f"{HOME}/bin/hermes")
            status("hermes", "installed, no key, not connected")
            sh("cua connect setup --agents hermes -y --json")
            status("hermes", "connected, no key")
            os.makedirs(f"{HOME}/.hermes", exist_ok=True)
            with open(f"{HOME}/.hermes/.env", "a") as f:
                f.write("OPENROUTER_API_KEY=sk-or-v1-test-not-real\n")
            status("hermes", "key in .env (signed in)")
            sh("cua connect setup --agents hermes --sign-in -y --json")
            status("hermes", "key copied to Keyvault")
            sh("cua connect remove --agents hermes --sign-in -y --json")
            status("hermes", "after Disconnect")
        else:
            RESULTS.append({"harness": "hermes", "cell": "install", "error": (p.stdout + p.stderr)[-500:]})
    except Exception as e:
        RESULTS.append({"harness": "hermes", "error": repr(e)})
    # Cursor CLI: its official installer (into ~/.local/bin).
    try:
        status("cursor", "not installed")
        p = sh("curl -fsSL https://cursor.com/install | bash", timeout=900)
        status("cursor", "installed, not connected")
        sh("cua connect setup --agents cursor -y --json")
        status("cursor", "connected (sign-in unknown)")
        sh("cua connect remove --agents cursor -y --json")
        status("cursor", "after Disconnect")
    except Exception as e:
        RESULTS.append({"harness": "cursor", "error": repr(e)})
    print(json.dumps({"results": RESULTS, "log": LOG[-60:]}, indent=1))
    shutil.rmtree(ROOT, ignore_errors=True)


main()
