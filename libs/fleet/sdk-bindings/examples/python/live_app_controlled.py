#!/usr/bin/env python3
import asyncio
import json
import os
import time
import urllib.error
import urllib.request

from cyclops_sdk import (
    CreateClaimRequest,
    CreatePoolRequest,
    CyclopsClient,
    CyclopsConfiguration,
    CyclopsCredentials,
    HttpClient,
    HttpHeader,
    HttpRequest,
    HttpResponse,
    PoolSpec,
    PoolTemplate,
    SandboxService,
)


class UrlLibHttpClient(HttpClient):
    async def execute(self, request: HttpRequest) -> HttpResponse:
        return await asyncio.to_thread(self._execute, request)

    def _execute(self, request: HttpRequest) -> HttpResponse:
        native = urllib.request.Request(
            request.url,
            data=request.body,
            method=request.method,
            headers={header.name: header.value for header in request.headers},
        )
        try:
            with urllib.request.urlopen(native, timeout=60) as response:
                return HttpResponse(
                    status=response.status,
                    headers=[HttpHeader(name=name, value=value) for name, value in response.headers.items()],
                    body=response.read(),
                )
        except urllib.error.HTTPError as error:  # lint-ignore: swallowed-exception
            return HttpResponse(
                status=error.code,
                headers=[HttpHeader(name=name, value=value) for name, value in error.headers.items()],
                body=error.read(),
            )


def pool_spec(image: str, image_pull_secret: str) -> PoolSpec:
    return PoolSpec(
        replicas=1,
        template=PoolTemplate(
            runtime=None,
            runtime_class_name=None,
            node_selector=None,
            tolerations=None,
            command=None,
            container_disk_image=image,
            image_pull_secret=image_pull_secret,
            cpu_cores=4,
            memory="4Gi",
            firmware=None,
            probes=None,
            oidc=None,
        ),
        autoscaling=None,
        services=[SandboxService(name="mcp", target_port=3000, protocol=None)],
    )


async def initialize_mcp(client: CyclopsClient, sandbox) -> int:
    body = json.dumps(
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "cyclops-uniffi-python", "version": "0.1.0"},
            },
        },
        separators=(",", ":"),
    ).encode()
    deadline = time.monotonic() + 300
    while True:
        response = await client.service_request(
            sandbox,
            "mcp",
            "/mcp",
            HttpRequest(
                method="POST",
                url="https://ignored.invalid/mcp",
                headers=[
                    HttpHeader(name="accept", value="application/json, text/event-stream"),
                    HttpHeader(name="content-type", value="application/json"),
                ],
                body=body,
            ),
        )
        if 200 <= response.status < 300:
            return response.status
        if response.status in (502, 503, 504) and time.monotonic() < deadline:
            await asyncio.sleep(5)
            continue
        raise RuntimeError(f"MCP initialize failed with HTTP {response.status}: {response.body!r}")


async def main() -> None:
    client_id = os.environ["CUA_CLIENT_ID"]
    client_secret = os.environ["CUA_CLIENT_SECRET"]
    base_url = os.environ["CUA_BASE_URL"]
    token_url = os.environ["CUA_TOKEN_URL"]
    namespace = os.environ["CYCLOPS_NAMESPACE"]
    image_pull_secret = os.environ["CUA_IMAGE_PULL_SECRET"]
    image = os.environ["CUA_IMAGE"]

    client = CyclopsClient.connect(
        CyclopsConfiguration(
            base_url=base_url,
            token_url=token_url,
            credentials=CyclopsCredentials(client_id, client_secret),
            pool_poll_interval_ms=5000,
            pool_poll_limit=120,
            claim_poll_interval_ms=5000,
            claim_poll_limit=120,
        ),
        UrlLibHttpClient(),
    )
    pool = None
    claim = None
    try:
        pool = await client.create_pool(
            CreatePoolRequest(namespace=namespace, spec=pool_spec(image, image_pull_secret))
        )
        claim = await client.create_claim(CreateClaimRequest(pool=pool, spec=None))
        sandbox = await client.wait_claim(claim)
        status = await initialize_mcp(client, sandbox)
        print(
            json.dumps(
                {
                    "namespace": namespace,
                    "pool": pool.metadata.name,
                    "claim": claim.metadata.name,
                    "sandbox": sandbox.name,
                    "mcp_status": status,
                },
                sort_keys=True,
            )
        )
    finally:
        if claim is not None:
            await client.delete_claim(claim)
        if pool is not None:
            await client.delete_pool(pool)


asyncio.run(main())
