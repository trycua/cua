# Cyclops multi-language SDK

Rust, TypeScript, Go, and Python expose typed `Pool`, `Claim`, and `Sandbox` resources. `connect` configures the Cyclops base URL and OAuth credentials once. The Rust core owns service routing, tokens, authorization headers, network execution, and one retry after `401`.

## Porcelain API

Pool operations:

```text
pool = create_pool({ namespace, spec: pool_spec })
list_pools(namespace)
get_pool(pool)
update_pool(pool)
delete_pool(pool)
```

Claim operations:

```text
claim = create_claim({ pool })
claim = create_claim({ pool, spec: claim_spec })  # optional advanced override
list_claims(namespace)
get_claim(claim)
update_claim(claim)
delete_claim(claim)
sandbox = wait_claim(claim)
```

`PoolSpec` and `ClaimSpec` come from the checked-in Kubernetes CRDs. `wait_claim` returns sandbox identity and the pool's available service names. It does not expose service endpoint URLs.

## Named service HTTP

Select a client with a `Sandbox` and a user-defined pool service name. Requests use relative paths only. Unknown services raise a structured error with the requested name and sorted available names.

Python returns an `httpx.Client` backed by an SDK transport:

```python
with sdk.service_client(sandbox, "mcp") as mcp:
    response = mcp.post("/mcp", json={"jsonrpc": "2.0", "method": "initialize"})
```

TypeScript returns a fetch-compatible function:

```ts
const mcp = sdk.serviceFetch(sandbox, "mcp")
const response = await mcp("/mcp", { method: "POST", body: JSON.stringify(payload) })
```

Go provides a `net/http`-style request adapter whose `Do` method executes through the core:

```go
mcp, err := sdk.ServiceClient(sandbox, "mcp")
request, err := mcp.NewRequest("POST", "/mcp", bytes.NewReader(body))
response, err := mcp.Do(request)
```

Rust provides an SDK-owned request builder:

```rust
let mut mcp = sdk.service_client(&sandbox, "mcp")?;
let response = mcp.post("/mcp").body(body).send()?;
```

Applications never construct service endpoint URLs or receive OAuth tokens. The SDK does not expose a generic authenticated URL client; service selection plus a relative path is the only application-controlled HTTP route surface.

## App-controlled sequence

Each example creates a pool with an `mcp` service, creates and waits for a claim, selects `mcp`, and sends MCP `initialize` to `/mcp`. Because binding resets the guest, examples retry transient `502`, `503`, and `504` responses for up to five minutes. Cleanup deletes the claim before the pool and then closes the SDK host.

```text
cyclops-cs/sdk-bindings/examples/python/app_controlled.py
cyclops-cs/sdk-bindings/examples/typescript/app-controlled.ts
cyclops-cs/sdk-bindings/go/examples/app_controlled.go
cyclops-cs/sdk-bindings/rust/examples/app_controlled.rs
```

## Live example validation

Build the shared Rust core runner and provide credentials through environment variables. Enter the client secret interactively so it does not enter shell history:

```sh
cargo build --locked --manifest-path cyclops-cs/sdk-core/Cargo.toml --bin cyclops-core-runner
export CYCLOPS_CORE_RUNNER="$(pwd)/cyclops-cs/sdk-core/target/debug/cyclops-core-runner"
export CUA_CLIENT_ID="<client-id>"
read -rsp "CUA client secret: " CUA_CLIENT_SECRET && export CUA_CLIENT_SECRET && printf '\n'
export CUA_IMAGE="296062593712.dkr.ecr.us-west-2.amazonaws.com/desktop-workspace@sha256:b9e74dbff4cc727c33ff4b8483bffa0860bb99213041c88f11260ac31db7628f"
export CUA_IMAGE_PULL_SECRET="ecr-credentials"
```

`CUA_BASE_URL` defaults to `https://run.cua.ai`. `CUA_TOKEN_URL` defaults to the production Cyclops OAuth endpoint. Run examples independently so each receives a generated namespace:

```sh
uv run --project cyclops-cs/sdk-bindings/python python cyclops-cs/sdk-bindings/examples/python/app_controlled.py
(cd cyclops-cs/sdk-bindings/typescript && npm run build && node dist-examples/app-controlled.js)
(cd cyclops-cs/sdk-bindings/go && go run ./examples)
cargo run --locked --manifest-path cyclops-cs/sdk-bindings/rust/Cargo.toml --example app_controlled
```

## Validation

```sh
(cd cyclops-cs/sdk-schema-generator && go run . -root ../.. -check)
cargo fmt --manifest-path cyclops-cs/sdk-core/Cargo.toml -- --check
cargo clippy --locked --manifest-path cyclops-cs/sdk-core/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path cyclops-cs/sdk-core/Cargo.toml
cargo clippy --locked --manifest-path cyclops-cs/sdk-bindings/rust/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path cyclops-cs/sdk-bindings/rust/Cargo.toml --all-targets
(cd cyclops-cs/sdk-bindings/typescript && npm ci && npm run typecheck && npm test)
(cd cyclops-cs/sdk-bindings/go && test -z "$(gofmt -l .)" && go vet ./... && go test ./...)
(cd cyclops-cs/sdk-bindings/python && uv run --with ruff==0.9.10 ruff check cyclops_sdk tests ../examples && uv run python -m unittest discover -s tests && uv build)
```
