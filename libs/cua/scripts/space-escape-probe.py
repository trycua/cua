#!/usr/bin/env python3
"""Red-team probe: run INSIDE a Space (for example `cua sb cp` it in and `cua sb exec NAME -- python3 probe.py`).

Tries the raw routes a sandboxed Space might use to reach the computer that runs it, and exits 1 if any is open:
the engine socket, WSL interop, the host's own addresses (the gateway, `host.docker.internal`) on ports the
daemon, the relay and Windows file sharing listen on, and credentials in the environment. The scoped channel
(CUA_SPAWN) is the one way out and is tested by `cargo test -p cua-daemon --lib children::`.
"""
import os
import socket
import subprocess
import sys

problems = []

for path in ("/var/run/docker.sock", "/run/docker.sock", "/run/WSL", "/mnt/c", "/mnt/wsl"):
    if os.path.exists(path):
        problems.append(f"{path} exists")

hosts = set()
try:
    hosts.add(socket.gethostbyname("host.docker.internal"))
except OSError:
    pass
try:
    for line in subprocess.run(["ip", "route"], capture_output=True, text=True).stdout.splitlines():
        if line.startswith("default"):
            hosts.add(line.split()[2])
except OSError:
    pass
PORTS = (22, 135, 445, 3389, 8080, 9000, 59000, 64907)
for h in sorted(hosts):
    for p in PORTS:
        s = socket.socket()
        s.settimeout(2)
        try:
            s.connect((h, p))
            problems.append(f"{h}:{p} is reachable")
        except OSError:
            pass
        finally:
            s.close()

for k, v in os.environ.items():
    if any(w in k.upper() for w in ("TOKEN", "SECRET", "PASSWORD", "API_KEY")) and k != "CUA_SPAWN_TOKEN_FILE":
        problems.append(f"credential in the environment: {k}")

# The daemon's own token and address are not in the Space.
home = os.path.expanduser("~/.cua")
for root, _, files in os.walk(home):
    if "agents" in root or "tools" in root:
        continue
    for f in files:
        if f in ("daemon.json", "daemon.lock", "device-key"):
            problems.append(f"{os.path.join(root, f)} is in the Space")

print("hosts probed:", ", ".join(sorted(hosts)) or "none")
if problems:
    print("ESCAPE ROUTES OPEN:\n  " + "\n  ".join(problems))
    sys.exit(1)
print("ok: no raw route out of this Space")
