import json
import logging
import os
import subprocess
import time
import urllib.error
import urllib.request
from typing import Any, NotRequired, TypedDict

import httpx

from .generated_crd import (
    ClaimSpec as ClaimSpec,
    ClaimSpecLifecycle as ClaimSpecLifecycle,
    PoolSpec as PoolSpec,
    PoolSpecAutoscaling as PoolSpecAutoscaling,
    PoolSpecService as PoolSpecService,
    PoolSpecServiceProtocol as PoolSpecServiceProtocol,
    PoolTemplate as PoolTemplate,
    PoolTemplateFirmware as PoolTemplateFirmware,
    PoolTemplateOidc as PoolTemplateOidc,
    PoolTemplateRuntime as PoolTemplateRuntime,
    SandboxTemplateRef as SandboxTemplateRef,
)

logger = logging.getLogger(__name__)


class ResourceMetadata(TypedDict):
    namespace: str
    name: str
    labels: NotRequired[dict[str, str]]


class Pool(TypedDict):
    apiVersion: str
    kind: str
    metadata: ResourceMetadata
    spec: dict[str, Any]
    status: NotRequired[dict[str, Any]]


class Claim(TypedDict):
    apiVersion: str
    kind: str
    metadata: ResourceMetadata
    spec: dict[str, Any]
    status: NotRequired[dict[str, Any]]


class Sandbox(TypedDict):
    namespace: str
    claim: str
    sandbox: str
    services: list[str]


class CreatePoolRequest(TypedDict):
    namespace: str
    spec: PoolSpec


class CreateClaimRequest(TypedDict):
    pool: Pool
    spec: NotRequired[ClaimSpec]


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_args):
        return None


class UnknownServiceError(ValueError):
    def __init__(self, requested: str, available: list[str]):
        self.requested = requested
        self.available = available
        super().__init__(
            f"unknown sandbox service {requested!r}; available services: {available!r}"
        )


class _ServiceTransport(httpx.BaseTransport):
    def __init__(self, sdk, sandbox: Sandbox, service: str):
        self._sdk = sdk
        self._sandbox = sandbox
        self._service = service

    def handle_request(self, request: httpx.Request) -> httpx.Response:
        path = request.url.raw_path.decode("ascii")
        if not path.startswith("/") or path.startswith("//") or "#" in path:
            raise ValueError(
                f"service request path must be relative and start with '/': {path}"
            )
        body = request.read()
        response = self._sdk._call({
            "op": "service_request",
            "sandbox": self._sandbox,
            "service": self._service,
            "request": {
                "method": request.method,
                "path": path,
                "headers": list(request.headers.multi_items()),
                "body": body.decode("utf-8") if body else None,
            },
        })
        return httpx.Response(
            response["status"],
            content=response["body"].encode(),
            request=request,
        )


class SDK:
    def __init__(self, configuration):
        self._configuration = configuration
        self._token = None
        self._process = subprocess.Popen(
            [os.environ.get("CYCLOPS_CORE_RUNNER", "cyclops-core-runner")],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
        )
        try:
            self._call({"op": "configure", "configuration": {"protocol_version": 7, "base_url": configuration["base_url"]}})
        except BaseException:
            self.close()
            raise

    def create_pool(self, request: CreatePoolRequest) -> Pool:
        return self._call({"op": "create_pool", "request": request})

    def list_pools(self, namespace: str) -> list[Pool]:
        return self._call({"op": "list_pools", "namespace": namespace})

    def get_pool(self, pool: Pool) -> Pool:
        return self._call({"op": "get_pool", "pool": pool})

    def update_pool(self, pool: Pool) -> Pool:
        return self._call({"op": "update_pool", "pool": pool})

    def delete_pool(self, pool: Pool) -> None:
        self._call({"op": "delete_pool", "pool": pool})

    def create_claim(self, request: CreateClaimRequest) -> Claim:
        return self._call({"op": "create_claim", "request": request})

    def list_claims(self, namespace: str) -> list[Claim]:
        return self._call({"op": "list_claims", "namespace": namespace})

    def get_claim(self, claim: Claim) -> Claim:
        return self._call({"op": "get_claim", "claim": claim})

    def update_claim(self, claim: Claim) -> Claim:
        return self._call({"op": "update_claim", "claim": claim})

    def delete_claim(self, claim: Claim) -> None:
        self._call({"op": "delete_claim", "claim": claim})

    def wait_claim(self, claim: Claim) -> Sandbox:
        return self._call({"op": "wait_claim", "claim": claim})

    def service_client(self, sandbox: Sandbox, service: str) -> httpx.Client:
        available = sorted(set(sandbox["services"]))
        if service not in available:
            raise UnknownServiceError(service, available)
        return httpx.Client(
            base_url="https://cyclops.invalid",
            transport=_ServiceTransport(self, sandbox, service),
        )

    def close(self):
        if self._process.poll() is None:
            self._process.stdin.close()
            self._process.terminate()
            self._process.wait()

    def _call(self, request):
        self._process.stdin.write(json.dumps(request) + "\n")
        self._process.stdin.flush()
        for line in self._process.stdout:
            message = json.loads(line)
            if "kind" not in message:
                if not message["ok"]:
                    raise RuntimeError(message.get("error", "core operation failed"))
                return message.get("value")
            try:
                value = self._dispatch(message)
                reply = {"ok": True, "value": value}
            except BaseException as error:
                logger.warning("Cyclops host call failed: %s", error)
                reply = {"ok": False, "error": {"transport": str(error)}}
            self._process.stdin.write(json.dumps(reply) + "\n")
            self._process.stdin.flush()
        raise RuntimeError("core runner exited")

    def _dispatch(self, message):
        kind = message["kind"]
        if kind == "http":
            request = message["request"]
            body = request.get("body")
            http_request = urllib.request.Request(request["url"], data=body.encode() if body is not None else None, headers=dict(request["headers"]), method=request["method"])
            opener = urllib.request.build_opener(_NoRedirect())
            try:
                with opener.open(http_request, timeout=30) as response:
                    return {"status": response.status, "body": response.read().decode()}
            except urllib.error.HTTPError as error:
                logger.debug("HTTP request returned status %s", error.code)
                return {"status": error.code, "body": error.read().decode()}
        if kind == "sleep":
            time.sleep(message["milliseconds"] / 1000)
            return None
        if kind == "now_ms":
            return int(time.time() * 1000)
        if kind == "acquire_oauth_credentials":
            oauth = self._configuration["oauth"]
            return {"token_url": oauth["token_url"], "client_id": oauth["client_id"], "client_secret": oauth["client_secret"]}
        if kind == "load_access_token":
            return self._token
        if kind == "store_access_token":
            self._token = message["token"]
            return None
        raise RuntimeError(f"unknown host call {kind}")


def connect(configuration):
    return SDK(configuration)
