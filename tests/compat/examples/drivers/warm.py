"""Makes one Space through the job gateway with a long timeout, then deletes
it, so the first example does not hit an SDK's 60 s default timeout while
the machine pulls its image and boots (see tests/compat/run_space.sh).
Usage: python warm.py e2b|daytona|modal"""

import json
import os
import sys
import time
import urllib.request

prov = sys.argv[1]
t0 = time.time()


def call(method, url, headers, body=None, timeout=900):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(
        url, data=data, method=method, headers={"Content-Type": "application/json", **headers}
    )
    with urllib.request.urlopen(req, timeout=timeout) as r:
        raw = r.read()
        return json.loads(raw) if raw else {}


if prov == "e2b":
    base, hdr = os.environ["E2B_API_URL"], {"X-API-KEY": os.environ["E2B_API_KEY"]}
    sb = call("POST", f"{base}/sandboxes", hdr, {"templateID": "base", "timeout": 120})
    sid = sb.get("sandboxID", "")
    call("DELETE", f"{base}/sandboxes/{sid}", hdr, timeout=120) if sid else None
elif prov == "daytona":
    base, hdr = os.environ["DAYTONA_API_URL"], {
        "Authorization": "Bearer " + os.environ["DAYTONA_API_KEY"]
    }
    sb = call("POST", f"{base}/sandbox", hdr, {})
    sid = sb.get("id", "")
    call("DELETE", f"{base}/sandbox/{sid}", hdr, timeout=120) if sid else None
else:
    print("no warm-up for", prov)
print(f"warm {prov} {time.time() - t0:.1f}s")
