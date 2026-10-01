# Node.js live lifecycle example

This example uses Node 18+ `fetch` to provide the SDK's `HttpClient` callback.
It creates a pool and claim, waits for a sandbox, calls `GET /health` on its
`mcp` service, then cleans up both resources in `finally`.

## Prerequisites

- Node.js 18 or newer.
- A host-native `libcyclops_sdk` cdylib colocated where the generated UniFFI
  Node runtime can load it. The parent binding README describes this requirement.
- An image exposing an MCP service on port `3000` that responds to `GET /health`.

## Configuration and run

| Variable | Required | Description |
| --- | --- | --- |
| `CYCLOPS_BASE_URL` | Yes | Cyclops control-plane base URL. |
| `CYCLOPS_TOKEN_URL` | Yes | OAuth token endpoint. |
| `CYCLOPS_CLIENT_ID` | Yes | OAuth client ID. |
| `CYCLOPS_CLIENT_SECRET` | Yes | OAuth client secret. |
| `CYCLOPS_NAMESPACE` | Yes | Namespace where resources are created. |
| `CYCLOPS_IMAGE` | Yes | Container-disk image for the pool template. |
| `CYCLOPS_IMAGE_PULL_SECRET` | No | Kubernetes image-pull secret name. |

```bash
export CYCLOPS_BASE_URL="https://cyclops.example"
export CYCLOPS_TOKEN_URL="https://auth.example/oauth/token"
export CYCLOPS_CLIENT_ID="example-client"
export CYCLOPS_CLIENT_SECRET="replace-me"
export CYCLOPS_NAMESPACE="default"
export CYCLOPS_IMAGE="registry.example/cyclops-mcp:latest"

cd cyclops-cs/sdk-bindings/ts-uniffi/examples
npm install
npm start
```

The native library must be present before Node loads `../index.ts`.
