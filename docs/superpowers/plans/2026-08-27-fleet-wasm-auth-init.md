# Fleet WASM Authentication and Initialization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make client-credentials OAuth work in the `@trycua/fleet` WASM package and make its synchronous UniFFI API safe immediately after import.

**Architecture:** Add a built-package Node regression that exercises the real WASM artifact through a scripted `HttpClient`. Initialize the WASM at ESM module evaluation and replace `std::time::Instant` with the cross-target `web_time::Instant` while leaving token semantics unchanged.

**Tech Stack:** Rust 1.97.0, UniFFI 0.31.0, `wasm32-unknown-unknown`, `web-time` 1.1.0, TypeScript/ESM, Node test runner, pnpm 10.12.3.

## Global Constraints

- Preserve the generated Rust and TypeScript public APIs.
- Keep `uniffiInitAsync()` exported and idempotent.
- Do not change OAuth request semantics, expiry skew, caching, or 401 refresh behavior.
- Do not require live Fleet credentials or external services in tests.
- Do not commit changes unless the user explicitly requests a commit.

---

### Task 1: Add Built-Package Regression

**Files:**
- Create: `libs/typescript/fleet/tests/node-client-credentials.test.mjs`
- Modify: `libs/typescript/fleet/package.json`

**Interfaces:**
- Consumes: Existing generated `CyclopsCredentials`, `CyclopsConfiguration`, `CyclopsClient.connect`, and `HttpClient` callback shape.
- Produces: `pnpm run test:node`, executed automatically after `build:artifacts` by `pnpm run build`.

- [x] **Step 1: Add the failing Node regression**

```js
import assert from 'node:assert/strict';
import test from 'node:test';

import {
  CyclopsClient,
  CyclopsConfiguration,
  CyclopsCredentials,
} from '../dist/node.js';

const encoder = new TextEncoder();

class ScriptedHttpClient {
  requests = [];

  async execute(request) {
    this.requests.push(request);
    if (request.url === 'https://identity.example/oauth/token') {
      return {
        status: 200,
        headers: [],
        body: encoder.encode(JSON.stringify({ access_token: 'token-a', expires_in: 3600 })).buffer,
      };
    }
    return { status: 200, headers: [], body: encoder.encode('[]').buffer };
  }
}

test('initializes before synchronous FFI calls and caches client-credentials tokens', async () => {
  const httpClient = new ScriptedHttpClient();
  const credentials = new CyclopsCredentials('client-id', 'client-secret');
  const configuration = CyclopsConfiguration.create({
    baseUrl: 'https://run.example',
    tokenUrl: 'https://identity.example/oauth/token',
    credentials,
    poolPollIntervalMs: 1n,
    poolPollLimit: 1,
    claimPollIntervalMs: 1n,
    claimPollLimit: 1,
  });
  const client = CyclopsClient.connect(configuration, httpClient);

  await client.listNamespaces();
  await client.listNamespaces();

  const tokenRequests = httpClient.requests.filter(
    ({ url }) => url === 'https://identity.example/oauth/token',
  );
  assert.equal(tokenRequests.length, 1);
  assert.equal(tokenRequests[0].method, 'POST');

  const controlPlaneRequests = httpClient.requests.filter(
    ({ url }) => url === 'https://run.example/api/namespaces',
  );
  assert.equal(controlPlaneRequests.length, 2);
  for (const request of controlPlaneRequests) {
    assert.equal(
      request.headers.find(({ name }) => name.toLowerCase() === 'authorization')?.value,
      'Bearer token-a',
    );
  }
});
```

- [x] **Step 2: Wire the regression into package builds**

Update `libs/typescript/fleet/package.json` scripts to:

```json
{
  "build": "pnpm run build:artifacts && pnpm run test:node",
  "build:artifacts": "node ./scripts/build.mjs",
  "test:node": "node --test ./tests/node-client-credentials.test.mjs",
  "pack:check": "pnpm run build && npm pack --dry-run"
}
```

- [x] **Step 3: Run the test to verify the initialization failure**

Run:

```bash
corepack pnpm --dir libs/typescript install --frozen-lockfile
corepack pnpm --dir libs/typescript/fleet build
```

Expected: the Node test fails before making HTTP requests with an uninitialized UniFFI error containing `rustcallstatus_new` or an equivalent undefined WASM binding.

---

### Task 2: Initialize WASM During Module Import

**Files:**
- Modify: `libs/typescript/fleet/scripts/build.mjs`
- Test: `libs/typescript/fleet/tests/node-client-credentials.test.mjs`

**Interfaces:**
- Consumes: Memoized `uniffiInitAsync(): Promise<void>` in each generated package entry.
- Produces: Both Node and browser ESM entry points finish WASM and UniFFI initialization before importers can use exported synchronous constructors.

- [x] **Step 1: Await initialization in both generated entries**

Append this statement after each `uniffiInitAsync` declaration in `browserSource` and `nodeSource`:

```ts
await uniffiInitAsync()
```

Keep `createFleetClient()` unchanged; its explicit await remains an idempotent compatibility path.

- [x] **Step 2: Rebuild and verify the test advances to token timing**

Run:

```bash
corepack pnpm --dir libs/typescript/fleet build
```

Expected: constructors and `CyclopsClient.connect()` succeed without an explicit initialization call, and the test now fails when the first OAuth token response reaches the WASM-incompatible `std::time::Instant::now()` path.

---

### Task 3: Use a WASM-Compatible Monotonic Clock

**Files:**
- Modify: `libs/fleet/Cargo.toml`
- Modify: `libs/fleet/sdk/Cargo.toml`
- Modify: `libs/fleet/sdk/src/transport.rs`
- Modify: `libs/fleet/Cargo.lock` if dependency resolution changes it
- Test: `libs/fleet/sdk/tests/oauth.rs`
- Test: `libs/typescript/fleet/tests/node-client-credentials.test.mjs`

**Interfaces:**
- Consumes: Existing `Instant::now()`, `Instant::checked_add`, and `Instant::checked_sub` token-expiry operations.
- Produces: The same operations through `web_time::Instant` on native and `wasm32-unknown-unknown` targets.

- [x] **Step 1: Add the workspace dependency**

Add to `[workspace.dependencies]` in `libs/fleet/Cargo.toml`:

```toml
web-time = "1.1.0"
```

Add to `[dependencies]` in `libs/fleet/sdk/Cargo.toml`:

```toml
web-time.workspace = true
```

- [x] **Step 2: Replace only the Instant import**

Change the imports in `libs/fleet/sdk/src/transport.rs` from:

```rust
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
```

to:

```rust
use std::{sync::Arc, time::Duration};
use web_time::Instant;
```

Do not alter the token cache, expiry skew, refresh, or generation logic.

- [x] **Step 3: Run focused native OAuth compatibility tests**

Run:

```bash
CARGO_PROFILE_TEST_DEBUG=0 cargo test --manifest-path libs/fleet/Cargo.toml -p cyclops-sdk --test oauth
```

Expected: all OAuth tests pass, confirming native cache and refresh semantics remain unchanged.

- [x] **Step 4: Build and run the real Node/WASM regression**

Run:

```bash
corepack pnpm --dir libs/typescript/fleet build
```

Expected: the built package test passes; it constructs the raw API without `uniffiInitAsync()`, performs one token request, performs two authenticated control-plane requests, and reuses `token-a`.

- [x] **Step 5: Run package and diff validation**

Run:

```bash
corepack pnpm --dir libs/typescript/fleet build
npm pack --dry-run
git diff --check
git status --short
```

Expected: the package dry-run succeeds, the diff has no whitespace errors, and only the planned source, test, dependency, lockfile, spec, and plan files are modified.
