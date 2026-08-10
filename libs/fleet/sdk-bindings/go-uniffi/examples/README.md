# Go live lifecycle example

This example creates a one-replica pool, creates a claim for that pool, waits
for a sandbox, calls its `mcp` service at `GET /health`, then deletes the claim
and pool. It is excluded from normal package builds with `//go:build ignore`.

## Prerequisites

- Go 1.22 or newer.
- A built `libcyclops_sdk` cdylib visible to cgo. See the parent binding README
  for `CGO_LDFLAGS` and runtime-loader examples.
- An image exposing an MCP service on port `3000` that responds to `GET /health`.

## Configuration

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

cd cyclops-cs/sdk-bindings/go-uniffi/examples
CGO_LDFLAGS='-L/path/to/native -lcyclops_sdk' LD_LIBRARY_PATH=/path/to/native go run main.go
```

The example always attempts to delete resources it created, including after a
failed wait or service request. Delete resources manually if termination prevents cleanup.
