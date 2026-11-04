# Hidden docs prelude `sb-web-local`: the sandbox `sb` a page's earlier section
# started, serving HTTP on the service `web` (port 8000). Local (container lane;
# combine with `local-cleanup`). Port 9222 is declared too, as the forwarding
# example forwards it.
from cua_sandbox import Image as _Image
from cua_sandbox import Sandbox as _Sandbox
from cua_sandbox import http as _http

sb = await _Sandbox.create(  # noqa: F704 - docs blocks allow top-level await
    _Image.from_registry("python:3.12-slim"),
    command=["python", "-m", "http.server", "8000"],
    services={"web": 8000, "devtools": 9222},
    wait_for=_http("web", "/"),
    local=True,
)
