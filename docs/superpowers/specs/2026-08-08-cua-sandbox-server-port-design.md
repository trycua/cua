# Cua Sandbox Fleet Server Port Design

## Problem

Fleet-backed `Sandbox.create()` and `Sandbox.ephemeral()` currently assume the
guest computer server listens on TCP port `8000`. The transport uses that value
for both the generated `server` Service and the VM readiness probe.

The canonical Linux `desktop-workspace` image listens on port `5000`, while the
Windows `cua-server-windows` image listens on port `8000`. A single hardcoded
port therefore prevents the public sandbox creation API from supporting both
images without changing their image contracts.

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

- the Fleet `server` Service `targetPort`; and
- the generated VM readiness probe TCP port.

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
tests. Perform one live E2E with the canonical Linux image using
`server_port=5000`, verifying screen access and owned-namespace cleanup.

## Documentation

Document the optional argument in the public constructor docstrings and add a
README example for the canonical Linux registry image:

```python
sandbox = await Sandbox.create(image, server_port=5000)
```

No image-name inference is added. The caller explicitly declares the guest
computer-server contract, avoiding brittle registry-specific behavior.
