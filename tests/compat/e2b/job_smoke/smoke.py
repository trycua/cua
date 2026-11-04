"""`cua run --compat e2b -- python smoke.py`: the unmodified E2B SDK inside a
Cua job Space, creating Cua Spaces through the job's gateway. Exits 1 on any
failure."""

import json
import os
import ssl
import time
import urllib.request

from e2b import Sandbox

results = {}
sb = Sandbox.create(timeout=600)
try:
    r = sb.commands.run("echo hello from $(hostname)")
    results["run"] = r.stdout.startswith("hello from")
    root = sb.commands.run("id -u", user="root")
    results["root"] = root.stdout.strip() == "0"
    sb.files.write("/tmp/www/index.html", "served-by-a-cua-space", user="root")
    results["files"] = sb.files.read("/tmp/www/index.html") == "served-by-a-cua-space"
    sb.commands.run(
        "cd /tmp/www && python3 -m http.server 8080", background=True, user="root", timeout=0
    )
    url = f"https://{sb.get_host(8080)}/"
    body = None
    for _ in range(30):
        try:
            with urllib.request.urlopen(
                url, timeout=5, context=ssl.create_default_context()
            ) as resp:
                body = resp.read().decode()
                break
        except Exception as e:  # noqa: BLE001
            body = repr(e)
            time.sleep(1)
    results["get_host_https"] = body == "served-by-a-cua-space"
finally:
    results["kill"] = sb.kill()
print(json.dumps(results))
raise SystemExit(0 if all(results.values()) else 1)
