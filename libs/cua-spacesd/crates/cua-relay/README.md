# cua-relay

The rendezvous that lets your devices reach machines you host (`cua host`)
without opening ports. It runs at `https://relay.cua.ai`; any self-hosted
`cua-relay` works the same way.

A hosted machine (`cua-spacesd join`) keeps one outbound WebSocket to the
relay, multiplexed with yamux. Each client connection becomes one stream on
it, spliced into the machine's local spacesd listener. Everything spacesd
serves flows through: native gRPC (h2), gRPC-Web, WebSockets (desktop and
media streams, port tunnels, hotspot) and `/files` transfers.

TLS terminates in front of the relay (ingress or load balancer), and the
relay parses every request it forwards (HTTP/1.1, h2/gRPC, WebSocket
upgrades) to route and authorize it. That means the relay process sees the
plaintext of everything it proxies while it is in flight: gRPC payloads,
file transfers, the desktop media stream, and any secret a caller sends
through it (see "Sealed delivery" below for the two payloads that need
stronger handling: Keyvault site-login passwords and teleport session
bundles). A compromised relay, relay host or a memory dump can read a live
session. See `docs/content/docs/spaces/guides/relay-security.mdx` for
the full, plain accounting of what the relay can and cannot see.

What the relay never stores (on disk or beyond the life of the request):

- **Env / account tokens in the clear.** In account mode the relay strips
  the client's account token and device-session header before forwarding,
  replacing them with a short-lived relay-signed assertion, so neither
  reaches the machine and neither is persisted. In static (self-hosted)
  mode clients present the env token end to end and the relay forwards it
  unmodified without persisting it; it still parses it in transit (see
  above).
- **Machine tokens in the clear.** The directory keeps only their SHA-256.
- **Device private keys.** Devices prove their key; the relay keeps the
  public key.
- **Traffic.** Streams are forwarded, not recorded (access logs are off:
  relay-minted tickets and signed URLs ride in query strings).

### Sealed delivery (Keyvault, teleport)

Keyvault site-login and teleport session-bundle deliveries are sealed to the
destination machine's own key (an established construction over a vetted
crate, never hand-rolled) so the relay forwards ciphertext for these two
payload kinds specifically, once the machine's key is pinned. A machine
whose image predates pinning has no such key: delivery over a `relay:` path
is refused unless the caller explicitly opts in per delivery, with a clear
warning that the relay could read the secret. Local and direct paths are
unaffected (no relay in the path). See `cua-machine-seal` and
`cua_teleport::send`/`cua_spaces_ext::daemon::keyvault` for the current
state of this work.

## How it works

```text
 your device                     relay.cua.ai                    hosted machine
 (cua, SDK, app)                                                 (cua-spacesd join)
      |                               |   <- GET /relay/v1/connect   |
      |                               |      (WSS, machine token)    |
      |  /m/<machine-id>/...  ------> |  owner / allow-list / device |
      |  account token +              |  checks, then one yamux      |
      |  x-cua-device-session         |  stream + signed assertion ->|
```

- **Registration.** `cua host setup` registers the machine with the owner's
  account token (`POST /v1/machines`) and gets a machine token back. The
  machine joins with it on `GET /relay/v1/connect` (headers
  `x-cua-machine-id`, `[a-z0-9-]{8,64}`, and `x-cua-env-driver-version`) and
  answers heartbeats; one without a heartbeat for `CUA_RELAY_IDLE_SECS` is
  dropped. It reconnects on its own with capped, jittered backoff.
- **Connection.** Clients reach a machine at `/m/<machine-id>/...` (path
  mode, prefix stripped) or `<machine-id>.<domain>` (host mode, with
  `CUA_RELAY_HOST_DOMAIN`).
- **Account auth.** Account tokens are OIDC access tokens from auth.cua.ai,
  checked against the issuer's JWKS, audience (`cua-relay`) and expiry. The
  relay then checks ownership, the machine's allow-list and its sharing
  switch, and forwards an EdDSA assertion (audience: the machine id, at most
  60 s) that the machine verifies with the relay's key
  (`/.well-known/jwks.json`, also sent on the join handshake).
- **Device enrollment.** A device that lists or reaches an account's
  machines holds a P-256 key (OS keychain, or a `0600` file) and is enrolled
  with a second factor: a fresh interactive sign-in (`auth_time` at most 10
  minutes old; for any device after the account's first, also a verified
  email), or approval from an enrolled device by one-time code or device
  id. Devices report a machine id (a hash of the host's hardware or install
  identity, stored keyed per account): when a new key of the same machine
  enrolls, it replaces the machine's older records (revoked as superseded,
  audited as `device_rekeyed`, left out of listings), so switching builds or
  losing a key does not leave duplicates. The machine id never enrolls
  anything on its own. Enrollment lasts 30 days, then one approval or fresh
  sign-in re-verifies it. Per session the device signs a timestamp
  (`POST /v1/devices/session`) and sends the token as `x-cua-device-session`.
  For 14 days after enrollment is first turned on, devices signed in from
  before keep access, flagged and audited.
- **Audit.** Every enrollment change, device session, machine access, share
  change, registration and sharing stop is recorded in the account's audit
  log (`GET /v1/audit`, `cua devices audit`), bounded and persisted with the
  devices.

The API: `/v1/machines` (directory, see `src/api.rs`), `/v1/devices` and
`/v1/audit` (see `src/device_api.rs`), `/v1/info`, `/healthz`, and
`/relay/v1/machines` (connected machines, admin token only).

A second, static mode serves self-hosters: machines join with a shared
registration token (`CUA_RELAY_TOKENS`) and clients present the env token
end to end. By default the relay forwards only requests that carry a
spacesd credential (bearer, ticket or signed URL).

## Security model

- **Hosting a machine exposes only that machine.** A machine token serves
  only its own machine: it reads its record, stops sharing (and undoes its
  own stop) and unregisters. It cannot list, rename, reach other machines,
  change the allow-list, register machines or manage devices. Hosting never
  enrolls the machine as a client.
- **Verified-email allow-lists.** Sharing is by account id or email (at most
  64 entries). An email only matches when the issuer marked it
  `email_verified`. Stop sharing cuts every client at once.
- **A stolen account session is not enough.** Listing or reaching machines
  also needs an enrolled device's session (`CUA_RELAY_DEVICE_ENROLLMENT=on`).
- **No URL logging.** Logs name machines, accounts and devices, never
  request paths or query strings (where tickets and signed URLs live).
  Machine `Set-Cookie` headers are dropped. Token comparisons are constant
  time.
- **A tamper-evident audit log (S9).** Every account audit entry is hash
  chained (each entry's `hash` covers the previous entry's `hash`; `mac` is
  an HMAC-SHA256 of it under a key generated on first use, 0600, next to
  the devices file, never derivable from the log file alone) the way the
  Keyvault's own audit log is. `GET /v1/audit` (200 events by default, up to
  `AUDIT_LIMIT`) and `GET /v1/audit?format=jsonl` (the full retained chain,
  one entry per line, for export to a SIEM or backup) both carry the chain
  fields. `AUDIT_LIMIT` (retention per account) is 5,000, up from an
  earlier 500. `cua_relay::devices::verify_audit_chain` checks a chain (or
  a whole account's, `DeviceStore::verify_audit_chain`); a copy of the
  devices file alone (a backup, a disk snapshot) cannot be edited
  undetectably, since reproducing a valid `mac` needs the separate audit
  key file.

## Machine origin (S6)

Path mode (`https://relay.cua.ai/m/<id>/…`) puts every machine's responses
on the *same* registrable origin as the relay's own account API and the
HTML5 viewer. A machine an attacker registers (S5 makes this harder, not
impossible) can then answer with content on that shared origin: a phishing
page, or a `Service-Worker-Allowed` response widening a worker's scope to
intercept later requests, including another machine's viewer page whose
`#ticket=` fragment it could then read.

Immediate hardening (shipped, applies in both modes; see
`harden_machine_response` in `src/server.rs`): `Service-Worker-Allowed` and
`Service-Worker` are stripped from every machine response; a response is
only let through as `text/html` on the one known viewer path (`/viewer`,
`cua_spacesd_html5::VIEWER_PATH`) -- everywhere else a machine's `text/html`
becomes `application/octet-stream` with `Content-Disposition: attachment`;
`X-Content-Type-Options: nosniff` is always set; a `Content-Security-Policy`
of `default-src 'none'; frame-ancestors 'none'` is added where the machine
sent none.

The stronger fix, also shipped in code: set `CUA_RELAY_HOST_DOMAIN` to a
**separate registrable domain** from the one the account API and viewer
live on (for example `m.relay.cua.ai` under `relay.cua.ai`'s parent zone,
or better, an entirely different domain such as `cua-relay.net`, the way
GitHub serves `github.dev`/`app.github.dev` off `github.com`). Once set:

- every machine gets `https://<id>.<host_domain>` (`Relay::machine_url`,
  what `GET /v1/machines` reports as each machine's `url`);
- path mode (`/m/<id>/…`) stops forwarding on the main origin entirely
  (`route()` refuses it with 404), so a machine's response is never reached
  by a request to `relay.cua.ai` itself, and cookies, storage and a Service
  Worker a rogue machine sets can only ever apply to *its own* subdomain,
  never to the relay's own pages or to a different machine's subdomain
  (browsers isolate by origin, and a subdomain is a different origin).

This needs a **DNS and TLS change outside this repo** (cloud, not code):

1. **DNS.** Add a wildcard record for the machine domain, for example
   `*.m.relay.cua.ai` (or the whole separate domain), pointing at the same
   Traefik/load balancer `relay.cua.ai` already uses. Keep it DNS-only /
   no-proxy, the way `relay.cua.ai` itself is set up, so the TLS handshake
   reaches Traefik directly.
2. **TLS.** Issue a wildcard certificate for `*.m.relay.cua.ai` (cert-manager
   `Certificate` with DNS-01, since HTTP-01 cannot prove a wildcard; the
   existing `relay.cua.ai` cert uses HTTP-01 and does not cover this). Add
   the wildcard host to the `IngressRoute`/`IngressRouteTCP` that fronts the
   relay pod, routed to the same Service.
3. **Deploy.** Set `CUA_RELAY_HOST_DOMAIN=m.relay.cua.ai` (or the chosen
   domain) on the relay Deployment and keep `CUA_RELAY_PUBLIC_URL` pointing
   at the account-API origin (`https://relay.cua.ai`) unchanged.
4. **Clients.** Nothing to change: clients already follow the `url` the
   directory API reports per machine (`Relay::machine_url`); they do not
   hardcode `/m/<id>`.

See the PR report for exact copy-pasteable manifest and DNS steps for this
deployment.

## Configuration

Every flag has an env var (`cua-relay --help`). Set at least one way to
register machines: `CUA_RELAY_TOKENS`/`CUA_RELAY_TOKEN_FILE` or
`CUA_RELAY_OIDC_ISSUER`.

| Env var | Default | Meaning |
|---|---|---|
| `CUA_RELAY_LISTEN` | `0.0.0.0:8080` | Listen address (plain HTTP/1.1 and h2c). |
| `CUA_RELAY_PUBLIC_URL` | from `Host` | Public base URL: assertion issuer and machine URLs. |
| `CUA_RELAY_OIDC_ISSUER` | unset | Enables account mode, e.g. `https://auth.cua.ai/realms/cyclops-cs`. |
| `CUA_RELAY_OIDC_JWKS_URL` | discovered | JWKS URL of the issuer. |
| `CUA_RELAY_OIDC_AUDIENCE` | `cua-relay` | Accepted audiences, comma-separated. |
| `CUA_RELAY_ACCOUNT_CLAIM` | `sub` | Token claim naming the account. |
| `CUA_RELAY_STATE_FILE` | in memory | Machine directory (JSON). |
| `CUA_RELAY_SIGNING_KEY_FILE` | fresh per process | Ed25519 assertion key, created on first use. |
| `CUA_RELAY_DEVICES_FILE` | next to the state file | Devices and audit log (`machines.json` gives `machines.devices.json`; in memory without a state file). Its audit hash-chain key (S9) is generated alongside it, 0600, at `<devices file>.audit-key`; back this file up with the devices file, or the chain from before a restore stops verifying (a fresh key generates rather than fail closed, since a lost audit key must not mean a lost relay). |
| `CUA_RELAY_MAX_MACHINES_PER_ACCOUNT` | `32` | Machines one account may register. |
| `CUA_RELAY_DEVICE_ENROLLMENT` | `on` | Require enrolled client devices (`on` / `off`). |
| `CUA_RELAY_DEVICE_TTL_DAYS` | `30` | Days an enrollment lasts. |
| `CUA_RELAY_DEVICE_GRACE_DAYS` | `0` (ended) | Days unenrolled devices keep access after enrollment is first on. Set this only for a time-boxed rollout window (S3): a stolen account token alone reaches every machine while it is open. |
| `CUA_RELAY_DEVICE_BOOTSTRAP_MAX_AUTH_AGE_SECS` | `600` | Max sign-in age that enrolls a device without an approval. |
| `CUA_RELAY_TOKENS` | unset | Static mode: comma-separated registration tokens. |
| `CUA_RELAY_TOKEN_FILE` | unset | Static mode: one token per line (`#` comments). |
| `CUA_RELAY_MAX_MACHINES_PER_TOKEN` | `16` | Static mode: machines per registration token. |
| `CUA_RELAY_ALLOW_ANONYMOUS_CLIENTS` | off | Static mode: also forward requests without a spacesd credential. |
| `CUA_RELAY_HOST_DOMAIN` | unset | Host mode: `<machine-id>.<domain>` routes to that machine, and path mode (`/m/<id>/…`) refuses on the main origin (S6; see "Machine origin" below). |
| `CUA_RELAY_ADMIN_TOKEN` | unset | Enables `GET /relay/v1/machines`. |
| `CUA_RELAY_MAX_STREAMS` | `256` | Concurrent client streams per machine (app-level check; S7). |
| `CUA_RELAY_MACHINE_MAX_STREAMS` | `256` | yamux's own per-machine stream cap (S7); keep `>=` the value above. |
| `CUA_RELAY_MACHINE_WINDOW_BYTES` | `67108864` (64 MiB) | yamux's per-machine connection receive window (S7); must be `>=` `machine-max-streams * 256 KiB`. |
| `CUA_RELAY_MAX_GLOBAL_STREAMS` | `4096` | Concurrent client streams across every machine at once: the relay-wide memory budget (S7). Tune to the pod's memory limit. |
| `CUA_RELAY_IDLE_SECS` | `90` | Seconds without a heartbeat before a machine is dropped (min 5). |
| `CUA_RELAY_SHUTDOWN_GRACE_SECS` | `10` | Seconds open requests get after SIGTERM / SIGINT. |
| `RUST_LOG` | `info` | Log filter. |

Connected machines live in the process, and the directory is a local file:
run one replica per hostname and replace it (Recreate) rather than rolling
it.

## Run and test

```bash
# From libs/cua-spacesd
cargo run -p cua-relay -- --tokens dev-token           # static mode on :8080
cargo run -p cua-relay --example fake_oidc -- \
  --listen 127.0.0.1:9000 --issuer http://127.0.0.1:9000 --sub ada --email ada@example.com
CUA_RELAY_OIDC_ISSUER=http://127.0.0.1:9000 cargo run -p cua-relay   # account mode

cargo test --locked -p cua-relay
```

`examples/fake_oidc.rs` is a fake issuer that completes every `cua auth login`
immediately and mints tokens for one user, for account-mode end to end runs. End to end in Docker:
`scripts/ci/relay-e2e.sh` (static mode, conformance through the relay) and
`libs/cua/crates/cua-spaces/tests/e2e/run-relay-account-e2e.sh` (account
mode). Client-side tests use `cua-fake-relay` (`cua-host`, feature
`testing`) instead of this crate.

## Build and deploy

```bash
docker build -f libs/cua-spacesd/crates/cua-relay/Dockerfile -t cua-relay libs
```

`deploy/relay.k8s.yaml` is an example manifest.
