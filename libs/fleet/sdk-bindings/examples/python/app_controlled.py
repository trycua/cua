#!/usr/bin/env python3
"""Application-controlled pool, claim, and sandbox operation flow."""
import json
import os
import time
import uuid

from cyclops_sdk import Claim, Pool, Sandbox, SDK, connect


def run_agent(sdk: SDK, sandbox: Sandbox):
    deadline = time.monotonic() + 300
    with sdk.service_client(sandbox, "mcp") as mcp:
        while True:
            response = mcp.post(
                "/mcp",
                headers={
                    "accept": "application/json, text/event-stream",
                    "content-type": "application/json",
                },
                json={
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2025-03-26",
                        "capabilities": {},
                        "clientInfo": {"name": "cyclops-sdk-python-example", "version": "0.1.0"},
                    },
                },
            )
            if 200 <= response.status_code < 300:
                return {"namespace": sandbox["namespace"], "claim": sandbox["claim"], "sandbox": sandbox["sandbox"], "mcp_status": response.status_code}
            if response.status_code not in (502, 503, 504) or time.monotonic() >= deadline:
                raise RuntimeError(f"MCP initialize failed with HTTP {response.status_code}: {response.text}")
            time.sleep(5)


def main():
    namespace = os.getenv("CYCLOPS_NAMESPACE", f"sdk-example-{uuid.uuid4().hex[:8]}")
    sdk = connect({
        "base_url": os.getenv("CUA_BASE_URL", "https://run.cua.ai"),
        "oauth": {
            "token_url": os.getenv("CUA_TOKEN_URL", "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token"),
            "client_id": os.environ["CUA_CLIENT_ID"],
            "client_secret": os.environ["CUA_CLIENT_SECRET"],
        },
    })
    pool: Pool | None = None
    claim: Claim | None = None
    try:
        pool = sdk.create_pool({
            "namespace": namespace,
            "spec": {
                "replicas": 1,
                "services": [{"name": "mcp", "targetPort": 3000, "protocol": "TCP"}],
                "template": {
                    "containerDiskImage": os.environ["CUA_IMAGE"],
                    "imagePullSecret": os.environ["CUA_IMAGE_PULL_SECRET"],
                    "cpuCores": 4,
                    "memory": "4Gi",
                },
            },
        })
        claim = sdk.create_claim({"pool": pool})
        sandbox = sdk.wait_claim(claim)
        result = run_agent(sdk, sandbox)
        print(json.dumps(result, sort_keys=True))
    finally:
        try:
            if claim is not None:
                sdk.delete_claim(claim)
        finally:
            try:
                if pool is not None:
                    sdk.delete_pool(pool)
            finally:
                sdk.close()


if __name__ == "__main__":
    main()
