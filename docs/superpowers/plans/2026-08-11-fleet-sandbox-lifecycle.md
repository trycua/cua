# Fleet Sandbox Lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route Fleet-backed `Sandbox.create()` and `Sandbox.ephemeral()` through reusable pools, with durable claim serialization and explicit disconnect/close semantics.

**Architecture:** `Pool.apply()` idempotently reconciles a content-addressed template and warm pool for supported registry-image configuration. `Pool.get()` resolves an existing pool without mutation, and `Pool.claim()` creates or reattaches a named claim and returns a connected `Sandbox`. `Sandbox` stores claim identity, reconnects from serialized state, renews its controller-enforced lifetime, and releases only the claim on close; pools remain reusable infrastructure.

**Tech Stack:** Python, pytest/pytest-asyncio, generated `cua-fleet` UniFFI bindings.

## Global Constraints

- `pool=` is read-only and mutually exclusive with `image`.
- Without `pool=`, `image` is required and creation performs `Pool.apply(image, replicas=1).claim(...)`.
- `name=` identifies the claim; `Sandbox.name` remains the bound sandbox name.
- Expose `claim_name` and `pool_name` separately.
- `disconnect()` closes transport/client resources only.
- `close()` idempotently releases the claim only; it does not delete a pool or template.
- `ephemeral()` calls `close()` on every exit path.
- Unsupported Fleet options raise `NotImplementedError`.
- Named claim retries reattach only when the existing claim belongs to the requested pool.
- Preserve contributor credit from commits `6f7c60404` and `0bd1b621f` when adapting their claim lifecycle work.

### Task 1: Add Private Claim State

**Files:**
- Modify: `libs/python/cua-sandbox/cua_sandbox/pool.py`
- Modify: `libs/python/cua-sandbox/cua_sandbox/transport/fleet.py`
- Test: `libs/python/cua-sandbox/tests/test_pool.py`

- [ ] Write failing tests for versioned JSON claim identity, bound identity, pool verification, client ownership, renewal, and 404-tolerant release.
- [ ] Run `uv run --project libs/python/cua-sandbox pytest libs/python/cua-sandbox/tests/test_pool.py -q` and verify the new tests fail.
- [ ] Adapt the submitted Lease implementation into private `_ClaimHandle`; do not export a public Lease noun.
- [ ] Add `owns_sdk` transport behavior so reattached Sandbox transports close their Fleet client on disconnect.
- [ ] Re-run the focused tests and verify they pass.

### Task 2: Add Pool Apply/Get/Claim

**Files:**
- Modify: `libs/python/cua-sandbox/cua_sandbox/pool.py`
- Modify: `libs/python/cua-sandbox/cua_sandbox/transport/fleet_cloud.py`
- Test: `libs/python/cua-sandbox/tests/test_pool.py`

- [ ] Write failing tests for deterministic `Pool.apply`, read-only `Pool.get`, claim naming, retry reattachment, wrong-pool conflict, service readiness, and unsupported image options.
- [ ] Run the pool tests and verify the new tests fail.
- [ ] Move Fleet template/pool request construction from `FleetCloudTransport` into `Pool.apply()` using generated builders.
- [ ] Hash image registry, CPU, memory, services, and replicas into a stable DNS-safe pool name when no explicit pool name is supplied.
- [ ] Implement `Pool.claim()` as a durable async operation returning `Sandbox`, not an async context manager.
- [ ] Re-run `test_pool.py` and `test_fleet_builder_usage.py`.

### Task 3: Route Sandbox Factories Through Pools

**Files:**
- Modify: `libs/python/cua-sandbox/cua_sandbox/sandbox.py`
- Create: `libs/python/cua-sandbox/tests/test_sandbox_fleet_lifecycle.py`

- [ ] Write failing tests for the existing-pool and image-apply dispatch branches.
- [ ] Test mutual exclusion, missing image, unsupported Fleet options, and preservation of local/legacy behavior.
- [ ] Extend `Sandbox.create()` with `pool`, `replicas`, `service`, `claim_spec`, and initial lifetime arguments.
- [ ] Refactor `Sandbox.ephemeral()` to call `Sandbox.create()` and terminate with `close()`.
- [ ] Run lifecycle, runtime, and VM-cleanup tests.

### Task 4: Add Reattach/Renew/Close

**Files:**
- Modify: `libs/python/cua-sandbox/cua_sandbox/sandbox.py`
- Modify: `libs/python/cua-sandbox/tests/test_sandbox_fleet_lifecycle.py`

- [ ] Write failing tests for `to_dict`, awaitable/context-managed `from_dict`, `keep_alive`, `disconnect`, and idempotent `close`.
- [ ] Ensure reattach resolves the live claim and verifies its pool instead of trusting cached service data.
- [ ] Compute renewal deadlines with timezone-aware UTC arithmetic.
- [ ] Raise `NotImplementedError` for lifecycle methods on unsupported providers.
- [ ] Run focused lifecycle and transport tests.

### Task 5: Align Sync API, Exports, And Docs

**Files:**
- Modify: `libs/python/cua-sandbox/cua_sandbox/sync/__init__.py`
- Modify: `libs/python/cua-sandbox/cua_sandbox/__init__.py`
- Modify: `libs/python/cua-sandbox/README.md`
- Test: `libs/python/cua-sandbox/tests/test_pool.py`

- [ ] Update sync Pool wrappers for durable `apply/get/claim` behavior.
- [ ] Keep generated request/builders public, but keep `_ClaimHandle` private.
- [ ] Document existing-pool, image-applied, ephemeral, serialization, and renewal examples.
- [ ] Document every Fleet `NotImplementedError` boundary.
- [ ] Run the full `libs/python/cua-sandbox/tests` suite.
