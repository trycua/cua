# Private typed envelope HTTP carrier

This carrier connects unary HTTP requests to `DriverEnvelopeReceiver`. It is
disabled unless the trusted launcher sets `CUA_DRIVER_ENVELOPE_HTTP_PORT` to a
nonzero port. It binds only to `127.0.0.1` and fails startup if binding fails.
No non-loopback binding option is provided.

This is an implementation contract, not a released Fleet setup guide. A later
image/service change must connect the loopback endpoint to Fleet's existing
authorized private named-service route. Do not expose this endpoint directly:
it does not authenticate callers. IDs and generations are lifecycle markers,
not credentials. The carrier introduces no bearer token or pairing scheme.

## Wire contract

All requests use HTTP/1.1. POST bodies require one valid `Content-Length`.
Responses are JSON with `Connection: close`; pipelined requests are not executed.
Browser-origin requests, transfer encoding, duplicate lengths, and query strings
are refused. There is no CORS support.

| Request                              | Body                              | Result                                                              |
| ------------------------------------ | --------------------------------- | ------------------------------------------------------------------- |
| `POST /v1/connections`               | `{}`                              | `connection_id`, `generation`, `public_session`, and `capabilities` |
| `POST /v1/connections/{id}/exchange` | Canonical `DriverRequestEnvelope` | Canonical `DriverResponseEnvelope`                                  |
| `POST /v1/connections/{id}/cancel`   | `{"request_id":"REQUEST_ID"}`     | `{"ok":true}`                                                       |
| `DELETE /v1/connections/{id}`        | Empty or `{}`                     | `{"ok":true}`                                                       |

Every connection-specific request requires `X-Cua-Driver-Generation` with the
value returned at creation. Missing generations fail with HTTP 400, absent
connections with 404, and mismatches with 409. A client must not reopen a
connection automatically after those failures.

Creation binds a Standard session with a one-hour maximum lifetime and a
five-minute idle lifetime. The immutable runtime ceiling still applies:
incompatible runtimes refuse creation. This slice does not inherit unrestricted
mode or accept permission modes, manifests, or arbitrary session options from
the wire. Ordinary typed calls use `session=None`; operations requiring a
session label use the returned `public_session`.

`capabilities` contains `minimum_envelope_version`, `maximum_envelope_version`,
and `supports_cancellation`. This carrier supports envelope version 1 and
cancellation. Independent `bind_session` requests are not implemented; the
returned canonical Driver root already owns one bound session.

## Resource and cleanup limits

- Request headers: 16 KiB; request body: 1 MiB.
- Response body: 16 MiB. An oversized exchange closes its session and returns
  `response_too_large` with unknown completion; it must not be replayed.
- Live connection entries: 64, reaped after five idle minutes.
- HTTP tasks: 64; exchanges: 32, admitted without an unbounded wait queue.
  Remaining task capacity permits cancellation and close during action load.
- Request reading and response writing: ten-second limits. Exchange deadlines
  and per-connection replay prevention are enforced by the receiver.

Close is idempotent and retains the closed request ledger until idle removal.
Idle reaping skips active exchanges. Shutdown cancels the listener and its
connection tasks and closes owned sessions. It does not shut down the shared
Driver runtime or computer-server, release a Fleet claim, or delete a pool.

## Verification boundary

Synthetic parser/routing tests cover malformed input, limits, generation
checks, raw results, close, active-session reaping, cancellation under full
exchange load, and oversized responses. Run them from `libs/cua-driver/rust`:

```sh
cargo test -p cua-driver --bin cua-driver driver_service_http --locked
```

These tests do not establish guest network isolation, Fleet authorization,
image packaging, real desktop effects, or compatibility with a released Python
client. Those remain explicit integration and qualification gates.
