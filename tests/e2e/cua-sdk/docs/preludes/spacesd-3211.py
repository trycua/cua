# Hidden docs prelude `spacesd-3211`: what the page's "Start a spacesd to try
# it" step does, so the connect examples reach a real cua-spacesd at
# http://127.0.0.1:3211 with CUA_ENV_TOKEN set. Container lane only; the
# container is removed when the block exits.
import atexit as _atexit
import os as _os
import secrets as _secrets
import subprocess as _subprocess
import time as _time
import urllib.request as _urlreq

_os.environ["CUA_ENV_TOKEN"] = _secrets.token_hex(24)
_name = f"cua-e2e-{_os.environ.get('CUA_E2E_RUN', 'docs')}-envbox"[:63]
_subprocess.run(["docker", "rm", "-f", _name], capture_output=True)
_subprocess.run(
    [
        "docker",
        "run",
        "-d",
        "--name",
        _name,
        "--shm-size=512m",
        "--memory=2g",
        "-e",
        "CUA_ENV_TOKEN",
        "-p",
        "127.0.0.1:3211:3211",
        "ghcr.io/trycua/linux:24.04",
    ],
    check=True,
    capture_output=True,
)
_atexit.register(lambda: _subprocess.run(["docker", "rm", "-f", _name], capture_output=True))
for _ in range(120):  # bounded: at most ~2 minutes
    try:
        _urlreq.urlopen("http://127.0.0.1:3211/", timeout=2)
        break
    except _urlreq.HTTPError:
        break  # the server answers (any status)
    except OSError:
        _time.sleep(1)
