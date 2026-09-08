# Cua Sandbox Fleet Server Port Design

## Correction — August 9, 2026

The attempted certification with pinned image
`desktop-workspace@sha256:b9e74...` disproved the original image assumption.
That image exposes cua-driver MCP on TCP `3000` and does not run the CUA
computer-server on TCP `5000`. Positive live E2E remains blocked until a
suitable image that serves the computer-server `/cmd` API is available.

## Problem

Fleet-backed `Sandbox.create()` and `Sandbox.ephemeral()` currently assume the
guest computer server listens on TCP port `8000`. The transport uses that value
for both the generated `server` Service and the VM readiness probe.

Guest images may run the CUA computer-server on a non-default port such as
`5000`, while Windows computer-server images retain the default `8000`. A
single hardcoded port prevents callers from declaring those differing guest
image contracts.

## Public API

Add an optional keyword argument to the asynchronous public constructors:

```python
await Sandbox.create(image, server_port=5000)

async with Sandbox.ephemeral(image, server_port=5000) as sandbox:
    ...
```

`server_port` defaults to `8000`, preserving existing behavior for Windows and
all current callers that omit the argument. The existing synchronous context-manager
facade is out of scope because it does not expose `Sandbox.create()` or
`Sandbox.ephemeral()`.

## Fleet Transport Behavior

Pass `server_port` through `Sandbox._create()` into `FleetCloudTransport`.
The transport stores the selected port and uses it consistently for:

- the Fleet `server` Service `targetPort`;
- the generated VM readiness probe TCP port; and
- `forward_tunnel(server_port)`, which must route through the named `server` Service.

Ports declared with `Image.expose()` remain additional Fleet services named
`port-<port>`. If an exposed port equals `server_port`, do not create a duplicate
additional service for that port.

Connecting to an existing Fleet pool remains unchanged because the pool's
template already defines its service target and readiness behavior.

## Validation

Reject invalid values before provisioning. `server_port` must be an integer in
the TCP port range `1..65535`; booleans are not accepted as integers.

This prevents malformed Fleet templates and gives callers a local, actionable
error before any namespace or resource is created.

## Tests

Add focused regression coverage that first fails against the hardcoded-port
implementation and then passes with the fix:

1. `Sandbox.create()` and `Sandbox.ephemeral()` forward `server_port` to
   `FleetCloudTransport`.
2. `FleetCloudTransport` defaults to port `8000`.
3. A custom port changes both `server.targetPort` and the readiness probe.
4. An exposed port equal to `server_port` is not duplicated.
5. Invalid ports fail before Fleet provisioning.

Run the existing Fleet transport, sandbox creation, formatting, and packaging
tests. Once a suitable Linux computer-server image is available, perform one
live E2E using `server_port=5000`, verifying screen access and owned-namespace
cleanup. The pinned `desktop-workspace@sha256:b9e74...` image is not suitable.

## Documentation

Document the optional argument in the public constructor docstrings and add a
README example showing a generic Linux computer-server image contract:

```python
sandbox = await Sandbox.create(image, server_port=5000)
```

No image-name inference is added. The caller explicitly declares the port where
the guest serves the CUA computer-server `/cmd` API, avoiding brittle
registry-specific behavior.
