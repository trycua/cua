# Typed guest envelope receiver

This is the receiver foundation for the narrow slice accepted in
[RFC #2512](https://github.com/trycua/cua/issues/2512#issuecomment-5579029576).
It is not a released guest endpoint or a Fleet connection recipe.

`DriverEnvelopeReceiver` dispatches the existing version-1 Driver envelopes
through a canonical `CuaDriverSession`. A trusted guest host creates the session
and chooses its options. Requests cannot choose permission modes or manifest
paths. The native result envelope is preserved, including structured actions,
verification results, images, and tool refusals.

## Carrier responsibilities

The receiver opens no listener and verifies no credentials. A carrier must:

- Reuse the environment's existing authenticated, authorized service route.
- Keep guest exposure private. Do not add a public listener or new credentials.
- Authorize access before creating or looking up a connection.
- Pin the returned connection generation and reject replacement; generation
  markers are not credentials or proof of per-claim authorization.
- Bound HTTP body size before parsing, connection count, pending requests, and
  idle lifetime. Close sessions when connections expire.
- Route cancel and close independently from an in-flight exchange.
- Never automatically replay actions or silently reconnect to another guest.

The following carrier/binding changes must supply these properties before any
Fleet or local-guest capability is advertised. An existing MCP endpoint does
not implement the typed envelope carrier.

## First-slice operations

The receiver accepts `metadata`, `list`, and `call`. The filtered `list` contains
only desktop operations admitted by this receiver: desktop/screen/cursor state,
mouse and keyboard actions, native menus, window placement, clipboard,
verification, and agent cursor controls. Platform support and native permission
checks still apply. A listed tool is not a cross-platform qualification claim.

The receiver deliberately refuses host session enumeration (`sessions_list`),
session creation/escalation/end tools, process/host management, recording, shell,
and files. Typed methods retain their canonical names and types, but this
first-slice connection does not support the entire in-process inventory.
The carrier owns the already-bound session; its close route is the cleanup API.

`screenshot_out_file`, `image_path`, and `file_path` must be absent or null.
Screenshots remain in native results, and clipboard text remains supported.
The receiver does not reinterpret a caller-local path as a guest path or
silently transfer files. Use Sandbox's separate execution and file APIs.

## Lifecycle and failure behavior

Requests have a maximum serialized size of 1 MiB, an opaque bounded identity,
and a deadline at most 120 seconds in the future. A connection retains up to
4096 request identities without eviction. Once full, callers must explicitly
open a new connection. Duplicate requests never execute again, even if their
first response was lost.

Calls on one connection serialize. Independent connections may run concurrently.
Cancellation or timeout before dispatch proves nonexecution. After dispatch,
the receiver reports unknown action completion and closes that session because
a native effect may outlive a dropped future. A lost exchange future also
closes the session. Cancellation cannot promise to undo input already delivered.

Closing a receiver is idempotent. It closes only its session, not the shared
Driver runtime, computer-server, Fleet claim, pool, or local VM. Existing
computer-server and Sandbox APIs are unchanged.

## Verification

From `libs/cua-driver/rust`, run:

```sh
cargo check -p cua-driver-sdk --locked
cargo test -p cua-driver-sdk --locked remote_receiver::tests -- --test-threads=1
```

Fake-executor tests observe dispatch counts, exact results, cancellation races,
generation mismatches, replay refusal, ledger capacity, and isolated cleanup.
They do not prove HTTP authorization, real guest desktop control, cross-platform
delivery, image packaging, or Fleet cleanup. Those require the subsequent
carrier and exact-candidate guest qualification steps.
