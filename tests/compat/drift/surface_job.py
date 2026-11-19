#!/usr/bin/env python3
"""Produce a surface fixture for an SDK release INSIDE a job Space (the
package is imported there, never on this machine):

    surface_job.py daytona py 0.225.0 > fixtures/daytona-py-0.225.0.json
    surface_job.py daytona ts 0.225.0
    surface_job.py modal py latest

Needs a `cua` (env CUA) and a running daemon. In CI the same tools run
directly on the disposable runner instead (compat-drift.yml)."""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
PKG = {
    ("daytona", "py"): ("daytona", ["daytona"]),
    ("daytona", "ts"): ("@daytona/sdk", []),
    ("modal", "py"): ("modal", ["modal", "modal.exception"]),
    ("modal", "ts"): ("modal", []),
    ("e2b", "ts"): ("e2b", []),
    ("e2b", "py"): ("e2b", ["e2b"]),
}


def main() -> int:
    prov, lang, ver = sys.argv[1:4]
    pkg, mods = PKG[(prov, lang)]
    spec = pkg if ver == "latest" else (f"{pkg}=={ver}" if lang == "py" else f"{pkg}@{ver}")
    job = Path(tempfile.mkdtemp(prefix="surface-", dir=os.environ.get("CUA_EXAMPLES_WORK")))
    try:
        for f in ("surface_py.py", "surface_ts.mjs"):
            shutil.copy(HERE / f, job / f)
        if lang == "py":
            body = f"""uv venv -q .venv && uv pip install -q --python .venv/bin/python '{spec}' >install.log 2>&1 || {{ cat install.log; exit 3; }}
echo @@SURFACE-BEGIN
.venv/bin/python surface_py.py {' '.join(mods)}
echo @@SURFACE-END"""
        else:
            body = f"""uv venv -q .nv && uv pip install -q --python .nv/bin/python 'nodejs-wheel>=22,<25' >node.log 2>&1 && export PATH="$PWD/.nv/bin:$PATH"
npm init -y >/dev/null && npm install --no-audit --no-fund --loglevel=error '{spec}' typescript@5 >install.log 2>&1 || {{ cat install.log; exit 3; }}
echo @@SURFACE-BEGIN
node surface_ts.mjs . '{pkg}'
echo @@SURFACE-END"""
        (job / "run.sh").write_text("set -e\n" + body + "\n")
        subprocess.run(["git", "init", "-q"], cwd=job, check=True)
        subprocess.run(["git", "add", "-A"], cwd=job, check=True)
        p = subprocess.run(
            [
                os.environ.get("CUA", "cua"),
                "run",
                "--kind",
                "container",
                "--on",
                "host:local",
                "--cpus",
                "1",
                "--memory",
                "2048",
                "--",
                "sh",
                "run.sh",
            ],
            cwd=job,
            capture_output=True,
            text=True,
            timeout=1200,
        )
        text = p.stdout + p.stderr
        text = re.sub(r"(?m)^\[rank 0 [^\]]*\] ", "", text)
        m = re.search(r"@@SURFACE-BEGIN\n(.*?)\n@@SURFACE-END", text, re.S)
        if not m:
            sys.stderr.write(text[-3000:])
            return 1
        sys.stdout.write(m.group(1) + "\n")
        return 0
    finally:
        shutil.rmtree(job, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
