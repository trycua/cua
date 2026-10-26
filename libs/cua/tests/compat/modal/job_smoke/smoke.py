"""`cua run --compat modal -- python smoke.py`: the unmodified Modal SDK inside
a Cua job Space, creating Sandboxes (Cua Spaces) through the job's gateway.
Exits 1 on any failure."""

import json
import ssl
import time
import urllib.request

import modal

results = {}
app = modal.App.lookup("job-smoke", create_if_missing=True)
image = modal.Image.from_registry("ghcr.io/trycua/linux:24.04")
sb = modal.Sandbox.create(
    "python3",
    "-m",
    "http.server",
    "8080",
    "--directory",
    "/etc",
    app=app,
    image=image,
    timeout=600,
    encrypted_ports=[8080],
)
try:
    p = sb.exec("sh", "-c", "echo hello from $(hostname)")
    results["exec"] = p.stdout.read().startswith("hello from")
    sb.filesystem.write_text("served", "/tmp/smoke.txt")
    results["files"] = sb.filesystem.read_text("/tmp/smoke.txt") == "served"
    t = sb.tunnels()[8080]
    body = None
    for _ in range(30):
        try:
            with urllib.request.urlopen(
                t.url, timeout=5, context=ssl.create_default_context()
            ) as r:
                body = r.read().decode()
                break
        except Exception as e:  # noqa: BLE001
            body = repr(e)
            time.sleep(1)
    results["tunnel_https"] = isinstance(body, str) and "Directory listing" in body
    d = modal.Dict.from_name("job-smoke-dict", create_if_missing=True)
    d["k"] = {"v": 1}
    results["dict"] = d["k"] == {"v": 1}
finally:
    sb.terminate()
    results["terminated"] = sb.poll() is not None
print(json.dumps(results))
raise SystemExit(0 if all(results.values()) else 1)
