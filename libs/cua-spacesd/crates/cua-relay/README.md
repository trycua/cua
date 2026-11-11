# cua-relay

The rendezvous that lets your devices reach machines you host (`cua host`)
without opening ports. It runs at `https://relay.cua.ai`; any self-hosted
`cua-relay` works the same way.

A hosted machine (`cua-spacesd join`) keeps one outbound WebSocket to the
relay, multiplexed with yamux. Each client connection becomes one stream on
it, spliced into the machine's local spacesd listener. Everything spacesd
serves flows through: native gRPC (h2), gRPC-Web, WebSockets (desktop and
media streams, port tunnels, hotspot) and `/files` transfers.

The relay is also the **coordinator of a WireGuard®-based mesh** (see
"The mesh" below): current Cua devices and machines reach each other end
to end encrypted, directly when possible, with the relay forwarding only
ciphertext when not. What follows about the HTTPS path applies to the
browser viewer, web or Cloudflare-hosted agents, older Cua versions, and
`CUA_MESH=off`.

On that HTTPS path, TLS terminates in front of the relay (ingress or load
balancer), and the relay parses every request it forwards (HTTP/1.1,
h2/gRPC, WebSocket upgrades) to route and authorize it. That means the relay
process sees the plaintext of everything it proxies while it is in flight:
gRPC payloads, file transfers, the desktop media stream, and any secret a
caller sends through it (see "Sealed delivery" below for the two payloads
that need stronger handling: Keyvault site-login passwords and teleport
session bundles). A compromised relay, relay host or a memory dump can read
a live session on that path. See
`docs/content/docs/spaces/guides/relay-security.mdx` for the full, plain
accounting of what the relay can and cannot see.

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

## The mesh

`GET /v1/mesh/connect` (`src/mesh.rs`) is the coordinator of an in-app,
userspace WireGuard®-based mesh (the node is the MIT `cua-mesh` crate; the
WireGuard protocol state machine is Cloudflare's BSD-licensed boringtun).
Every device and machine keeps one WebSocket here:

1. **Who.** A machine authenticates as on `/relay/v1/connect` (its machine
   token, or a static registration token, plus `x-cua-machine-id`); a client
   device with its account token and enrolled-device session; on a static
   relay a client may connect anonymously and peers with nothing until a
   machine accepts it. The relay then sends a challenge, and the node proves
   it holds its WireGuard key (HMAC over the nonce keyed by X25519 between
   the node key and a per-connection key).
2. **Peer rules.** The relay computes each node's network map from what it
   already manages: a device peers with the machines its account owns or
   was shared, while sharing is on and the device is enrolled; a machine
   sees each such device with the identity an assertion would carry
   (account, verified email, `owner` / `shared` / `viewer`). Maps are
   recomputed on every change and every 2 s, and a device dropped from a map
   is cut at once. Static machines pair instead: a client sends a proof
   (`HMAC(env token, machine id | client key | ts)`), the relay forwards it,
   and the machine checks it, remembers the client, and ignores any peer it
   did not approve.
3. **Endpoints.** Nodes report their LAN addresses and the public address
   STUN (`--mesh-stun-listen`) shows them; the relay distributes them, and
   nodes hole-punch direct paths with sealed disco pings.
4. **Relaying.** Binary frames `dst key | packet` are forwarded between two
   peers only, and only WireGuard or disco packets (ciphertext; the relay
   holds no session key). `--mesh-relay off` stops relaying.

The registry (node keys, endpoints, pairings) persists in
`<state>.mesh.json`. Direct sessions do not depend on the relay at all, and
relayed ones resume when it is back. `/v1/info` advertises
`mesh: {version, stun}`; nodes use the mesh only when versions match and
fall back to the HTTPS path otherwise.

On a machine, cua-spacesd-server's `mesh_host` runs the node next to the relay join: an
account machine puts a peer's identity on each request as an assertion
signed with an in-process key it trusts next to the relay's (only on
requests carrying the per-forward session key the dialer chose), so the
spacesd host policy decides as for relayed calls; a static machine splices
streams unchanged (the env token rides inside WireGuard). Mesh UDP 3212
(`--mesh-udp-port`) reaches the local QUIC media port; it is a port of the
mesh's own network, so another cua-spacesd on the same computer does not
compete for it.

## Hosted-machine proxy transport

The host join uses `HTTPS_PROXY` for `wss`/`https` relay URLs and `HTTP_PROXY`
for `ws`/`http`, with `ALL_PROXY` as fallback. Each uppercase variable takes
precedence over its lowercase spelling, including when empty; an empty
scheme-specific value still permits `ALL_PROXY`. `NO_PROXY`/`no_proxy` uses
hyper-util's domain, IP and CIDR matching, without resolving names for CIDRs.
There is no automatic localhost bypass. Bypass is evaluated before validating
the selected proxy. These settings must be inherited by the join process.

This host transport supports unauthenticated HTTP proxies returning status 200
to CONNECT. HTTPS/SOCKS proxies, proxy URL credentials, paths and queries are
rejected. Proxy use in a CGI environment (`REQUEST_METHOD` present) is rejected.
A selected proxy failure never falls back to a direct connection. Native origin
TLS verification is unchanged, including platform trust configuration such as
`SSL_CERT_FILE` on OpenSSL platforms. HTTP CONNECT itself is not encrypted;
use a `wss` relay URL for origin encryption. A `ws` origin remains plaintext
inside the tunnel. Controller/SDK proxy support is separate from this host path.

The existing 20-second connection deadline and reconnect backoff also cover
proxy dialing and CONNECT. Diagnostics report the failed stage or HTTP status
without printing proxy credentials, relay URL queries or response bodies.

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
  machines holds a P-256 key (OS keychain, or a `0600` file) and is
  enrolled by a fresh interactive sign-in (`auth_time` at most 10 minutes
  old, plus proof of the device key) or by approval from an enrolled device
  (one-time code or device id). By default a fresh sign-in enrolls any
  device of the account, the way signing in adds a device to a tailnet:
  whoever can complete the account's sign-in already is the account, so the
  defences sit at sign-in (the identity provider's MFA) and in what a
  stolen credential cannot do here -- a refreshed or long-lived token never
  enrolls, a token without the device key opens no session, every
  enrollment is audited with how it happened (a new device by sign-in is
  `device_enrolled` / `bootstrap:fresh-sign-in`, "new device of the
  account"), and any enrolled device revokes another at once.
  `CUA_RELAY_DEVICE_REQUIRE_APPROVAL=true` restores the strict policy: a
  fresh sign-in then enrolls only the account's first device, a re-key of a
  machine it already has, or (verified email) a sign-in proving MFA; other
  new devices wait for an approval. `CUA_RELAY_DEVICE_REQUIRE_APPROVAL=phones`
  applies that strict rule to phone-class devices only (a device whose
  sign-in token was issued to a `CUA_RELAY_PHONE_CLIENTS` client, or that
  reports a phone platform, including the 30-day re-verification); every
  other device (the CLI, the desktop app, per-browser devices of the account
  app, unattended device-code installs) keeps the default policy. The relay
  refuses to start with `phones` unless `CUA_RELAY_PUSH=on` and at least one
  phone client is set. Devices report a machine id (a hash of
  the host's hardware or install identity, stored keyed per account): when
  a new key of the same machine enrolls, it replaces the machine's older
  records (revoked as superseded, audited as `device_rekeyed`, left out of
  listings), so switching builds or losing a key does not leave duplicates.
  The machine id never enrolls anything on its own. Enrollment lasts 30
  days and slides while the device is in use (each session moves the end to
  30 days from then); a device unused for 30 days needs a fresh sign-in or
  one approval again. Per session the device signs a timestamp
  (`POST /v1/devices/session`) and sends the token as `x-cua-device-session`.
  During a migration grace period (`CUA_RELAY_DEVICE_GRACE_DAYS`, none by
  default), devices signed in from before keep access, flagged and audited.
- **Audit.** Every enrollment change, device session, machine access, share
  change, registration and sharing stop is recorded in the account's audit
  log (`GET /v1/audit`, `cua devices audit`), bounded and persisted with the
  devices.

- **Pairing codes** ("add a machine" by link or QR). An enrolled device
  mints a one-time code (`POST /v1/pairing-codes`, 12 Crockford base32
  characters, 30 minutes by default, at most 24 hours, at most 10 unused
  per account); the machine being added registers with
  `authorization: Bearer cpc_<code>` instead of an account token, joins
  the account (confirmed, since an enrolled device minted the code; a
  code a team's owner or admin minted for the team also adds the machine
  to that team), and the code is spent. A revoked minting device voids
  its codes. Only the code's SHA-256 is stored; the owner lists
  (`GET /v1/pairing-codes`, never the code) and revokes
  (`DELETE /v1/pairing-codes/{id}`) them. See `src/pairing.rs`.

Removing a computer (`DELETE /v1/machines/{id}`) keeps the Spaces it provided
(records whose `host` is that id) but detaches them: they are not listed and
connections are refused (403, "its computer was removed from the account")
until the computer registers again with the same id.
`DELETE /v1/machines/{id}?spaces=delete` (owner) forgets them too and cuts
their connections. A computer can be removed while it is offline, so the
relay first keeps a pending deletion for it (the Spaces' ids and names, who
asked, the computer's token hash), and the computer deletes them, files
included, the next time it dials the relay:

- `GET /v1/machines/{id}/pending-deletions` lists them (`{computer, spaces:
  [{id, name}], requested_at, expires_at}`; empty `spaces` when none).
- `POST /v1/machines/{id}/pending-deletions/{space}/ack` marks one deleted
  (idempotent); the last ack clears the record.
- Only that computer's machine token is accepted: the one it had when it was
  removed, or its new one if it was added again with the same id. Another
  computer's token or an account token gets 403, an unknown token 401.
- Adding the computer again keeps the record (deleted means deleted), and a
  pending Space id cannot be registered again (410) until it is acked.
- A record expires 90 days after the (latest) request, so a computer that
  never comes back does not pin it forever; erasing the account drops it.

See `src/pending_deletions.rs`; the computer side is
`cua_spacesd_server::pending_deletions`.

Each account has a default Space and a fallback order, so the apps, CLI,
phone and agents agree where to work: `GET`/`PUT /v1/spaces/settings` and
`GET /v1/spaces/default?here=<machine-id>`, which resolves the default, then
the fallback (or every online computer, oldest first), then `here` (see
`src/spaces.rs`).

The API: `/v1/machines` (directory, see `src/api.rs`), `/v1/pairing-codes`, `/v1/devices` and
`/v1/audit` (see `src/device_api.rs`), `/v1/info`, `/healthz`, and
`/relay/v1/machines` (connected machines, admin token only).

A second, static mode serves self-hosters: machines join with a shared
registration token (`CUA_RELAY_TOKENS`) and clients present the env token
end to end. By default the relay forwards only requests that carry a
spacesd credential (bearer, ticket or signed URL).

## Security model

- **Hosting a machine exposes only that machine.** A machine token serves
  only its own machine: it reads its record, stops sharing (and undoes its
  own stop) and unregisters. It cannot list, rename, reach other computers,
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
live on, the way GitHub serves `github.dev` off `github.com`. The hosted
relay uses `m.cua.sh` (`relay.cua.ai` keeps the account API; staging uses
`m.staging.cua.sh`). A domain of its own matters twice: a page on it can
never set cookies for the account domain, and if someone shares something
malicious, reputation systems flag the share's domain, not the account
domain. Listing the host domain on the Public Suffix List (as `m.cua.sh` is
requested to be) also stops one share's origin from setting cookies for
every other share. Once set:

- every machine gets `https://<id>.<host_domain>` (`Relay::machine_url`,
  what `GET /v1/machines` reports as each machine's `url`), and each share
  its own origin under it (below);
- path mode (`/m/<id>/…`) stops forwarding on the main origin entirely
  (`route()` refuses it with 404), so a machine's response is never reached
  by a request to the relay's own origin, and cookies, storage and a Service
  Worker a rogue machine sets can only ever apply to *its own* subdomain,
  never to the relay's own pages or to a different machine's subdomain
  (browsers isolate by origin, and a subdomain is a different origin).

**Keeping path mode (`CUA_RELAY_KEEP_PATH_MODE=on`).** Clients that build
`<relay>/m/<id>` URLs themselves (older `cua` builds, and anything that
saved such a URL) keep working when this is on: `/m/<id>/…` is still
forwarded on the main origin, with every response hardened as above (no
cookies, no rendered HTML outside the viewer). The one exception is an old
path-mode share link opened by a browser: a request for
`/m/<id>/share/<key>/…` or `/m/<id>/share/pub/<token>/…` that carries no
account credential header gets a `308` to the same path on the share's own
origin (`<s|p><key>-<id prefix>.<host_domain>/…`). A tool that sends a
bearer token keeps getting path mode. The hosted relay runs with this on
while older clients are in use; turning it off is the full S6 setting.

Deploying a host domain needs DNS and TLS outside this repo:

1. **DNS.** A wildcard record for the machine domain (for example
   `*.m.cua.sh`) pointing at the same Traefik/load balancer the relay's own
   origin uses. Keep it DNS-only / no proxy, the way `relay.cua.ai` is set
   up, so the TLS handshake and long-lived WebSockets reach Traefik
   directly.
2. **TLS.** A wildcard certificate for it (cert-manager `Certificate` with
   DNS-01, since HTTP-01 cannot prove a wildcard). Add the wildcard host to
   an `IngressRoute` that fronts the relay pod (gRPC to the h2c scheme like
   the main origin), routed to the same Service.
3. **Deploy.** Set `CUA_RELAY_HOST_DOMAIN` (and `CUA_RELAY_KEEP_PATH_MODE=on`
   while clients still use path mode) on the relay Deployment and keep
   `CUA_RELAY_PUBLIC_URL` pointing at the account-API origin unchanged.
4. **Clients.** Share links follow the `url` the directory API reports per
   machine; nothing to change.

## Public TCP shares

A raw TCP service in a Space (a game server such as Minecraft Java on 25565)
can be shared publicly: game protocols carry no TLS or SNI, so each share
gets its own port from a pool and, optionally, an SRV record so people type
a name instead of a port. Off unless `CUA_RELAY_TCP_PORTS` is set; the cua
daemon also keeps it behind an experiment switch.

| Variable | Meaning |
|---|---|
| `CUA_RELAY_TCP_PORTS` | The pool, `26000-26063` |
| `CUA_RELAY_TCP_HOST` | The host people connect to (`tcp.m.cua.sh`), and the SRV target |
| `CUA_RELAY_TCP_PROXY_PROTOCOL` | `on` when a TCP proxy in front sends PROXY protocol v2 (client addresses for the limits); connections without it are refused |
| `CUA_RELAY_TCP_PER_ACCOUNT`, `CUA_RELAY_TCP_PER_MACHINE` | Ports held at once (2, 4) |
| `CUA_RELAY_TCP_CONN_RPS_PER_IP`, `CUA_RELAY_TCP_CONN_BURST_PER_IP` | New connections from one address (2/s, burst 10) |
| `CUA_RELAY_TCP_MAX_CONNS`, `CUA_RELAY_TCP_BYTES_PER_SEC` | Per share: open connections (64), bytes per second both ways (4 MiB) |
| `CUA_RELAY_TCP_MAX_TTL_HOURS` | Longest share (24) |
| `CUA_RELAY_TCP_SRV_DOMAIN`, `CUA_RELAY_DNS_ZONE_ID`, `CUA_RELAY_DNS_API_TOKEN` | SRV names (`_minecraft._tcp.mc-<hex>.m.cua.sh`) in a Cloudflare zone, with a token scoped to it |

The computer's daemon allocates a port (`POST /v1/machines/{id}/tcp-shares`
with the share's 40-hex token and the Space port), renews its lease every few
minutes (`…/{port}/keepalive`) and closes it (`DELETE …/{port}`). Each
connection to the port becomes a WebSocket to the share's public path on the
machine, opened through the relay's own router, so every rule of a public
link applies, and the daemon's share gateway splices it to the one Space
port the share names. Admin and remote-access ports (SSH, RDP, VNC, RCON
25575, Docker and Kubernetes APIs, common databases) are refused.

A reconcile sweep (at start and every 30 s) closes ports whose lease or end
passed, or whose machine left the directory or stopped sharing, restarts
listeners after a restart (allocations persist in `<state>.tcp.json`), and
makes the zone's SRV records carrying this relay's tag
(`cua-relay tcp-share <srv domain>`) match the live allocations exactly.
Every open and close is a row in the owner's audit log (`tcp_share_opened`,
`tcp_share_closed` with the reason).

## Share links (`cua share`)

`cua share` hands out links to a port or a folder on a machine of the
account. The machine's daemon serves them; the relay only routes and gates.
Needs `CUA_RELAY_HOST_DOMAIN` for browsers (see the S6 reasoning above);
path mode carries the same links for tools that send a bearer token but
never renders their HTML.

**Share origins.** In host mode every share gets its own origin, one DNS
label under the host domain, so a shared app sees its own root, root-relative
links work, and one share's cookies and storage never reach another share or
the machine's own origin:

| Label | Share | Machine sees |
|---|---|---|
| `s<16 hex>-<machine hex>.<domain>` | private or team (needs the account) | `/share/<id>/<path>` |
| `p<40 hex>-<machine hex>.<domain>` | public (the 160-bit token is the credential) | `/share/pub/<token>/<path>` |

`<machine hex>` is a prefix of at least 12 hex characters of the machine id
(the whole id up to the 63-character label limit); it must name exactly one
machine, else 404. On a share origin the relay serves none of its own routes
(`/healthz`, `/v1/...`): every path belongs to the app, except
`/_cua/session`. The relay sets `x-forwarded-host` and `x-forwarded-proto`
(replacing any client value) and removes `x-cua-share-caller`, the account
credentials and its own cookie before the request reaches the machine. On
the machine's own origin or in path mode, `/share/…` paths are forwarded
with the usual prefix and the same checks, but their HTML stays blocked.

**Public links** (`/share/pub/…`) need no account, on any origin form. They
still require the machine to be sharing (`stop-sharing` ends them), count
against the stream caps, never carry an assertion, and are rate limited by a
token bucket per client address (the last `X-Forwarded-For` entry, else the
socket peer) and per machine: `429` with `Retry-After`. The daemon behind
the machine checks the token's expiry and revocation.

**Browser sign-in.** A phone has no cua client, so a browser that opens a
private share origin without a credential gets `302 /v1/web/login`, an OIDC
authorization-code + PKCE sign-in against `CUA_RELAY_OIDC_ISSUER` (discovery
from its well-known document; the access token must pass the same validation
as any account token, so the web client needs the `cua-relay` audience).
`/v1/web/callback` exchanges the code, validates the token and redirects to
the share origin's `/_cua/session` with a 60-second, single-use,
relay-signed code bound to that origin. `/_cua/session` sets
`cua_share_session`: HttpOnly, host-only, `SameSite=Lax`, `Secure` on https,
at most 12 hours, a relay-signed identity bound to the machine id, the share
id and the share's host. The relay accepts it as the account credential on
that one private share origin and nowhere else: not on a public share, the
machine's own origin, path mode, gRPC or any other route. Every request still
runs the machine-role, team and revocation checks a bearer token runs; the
machine sees an ordinary relay assertion. The sign-in only ever returns to a
private share origin of this relay on an existing machine, and `to` can only
be a path on that origin.

**Device enrollment (a deliberate decision).** A browser has no enrolled
device, so a share session skips the device-enrollment gate. It is limited
to that one cookie on that one origin: it reaches only the HTTP of a share
its owner chose to hand out, never a desktop, a shell or any gRPC, and the
sign-in itself is the account's own OIDC login with whatever second factor
the realm demands. Bearer-token callers keep the device gate everywhere.

**Share origin responses.** HTML renders (this origin holds nothing else).
`Domain=` is removed from the app's `Set-Cookie` (host-only), the relay's
own cookie cannot be set or cleared by the app, service-worker widening
headers are dropped, and `X-Content-Type-Options: nosniff`,
`Referrer-Policy: no-referrer`, `X-Robots-Tag: noindex, nofollow` are always
set; a `Content-Security-Policy: frame-ancestors 'self'` is added when the
app sent no policy (the app's own policy is never replaced).

**Audit.** Each outcome is one `cua_relay::auth` line at `info` with
`event` and a coarse `detail`: `web_login_ok`, `web_login_failed`,
`web_login_refused`, `web_session_issued`, `web_session_refused`,
`public_rate_limited`. Like every line there: no token, email, account or
machine id, no address. Opens of the share itself are in the machine's
access log and the daemon's share audit.

The assertion gains an optional `teams` claim (the teams or organizations
the account reached the machine through; absent for the owner and a direct
share), which the daemon uses for team shares. Verifiers that predate it
ignore it.

## Optional hosted-relay features

The hosted relay (relay.cua.ai) is built with optional features (Cargo
feature `hosted`): allowance metering against the Cua plans, Teams, and the
routes the hosted Spaces API calls. The default build has none of them:
every account is Free and nothing is metered or refused for an allowance.
`GET /v1/usage` answers `404` ("metering is not enabled on this relay"), and
the Teams routes (`/v1/teams`, `/v1/teams/*`, `/join/team/{code}`) answer
`410` (`{"error": "gone", "detail": "Teams are not available on this
relay"}`). The apps and the CLI show these as "no allowance applies" and
"Teams are not available". See `src/free.rs`. The hosted features are in this
repository too (`HOSTED.md`): build with `--features hosted`, or the Docker
image with `--build-arg CUA_RELAY_HOSTED=1`.

## Configuration

Every flag has an env var (`cua-relay --help`). Set at least one way to
register machines: `CUA_RELAY_TOKENS`/`CUA_RELAY_TOKEN_FILE` or
`CUA_RELAY_OIDC_ISSUER`.

| Env var | Default | Meaning |
|---|---|---|
| `CUA_RELAY_LISTEN` | `0.0.0.0:8080` | Listen address (plain HTTP/1.1 and h2c). |
| `CUA_RELAY_PUBLIC_URL` | from `Host` | Public base URL: assertion issuer and machine URLs. |
| `CUA_RELAY_OIDC_ISSUER` | unset | Enables account mode, e.g. `https://auth.cua.ai/realms/<realm>`. |
| `CUA_RELAY_OIDC_JWKS_URL` | discovered | JWKS URL of the issuer. |
| `CUA_RELAY_OIDC_AUDIENCE` | `cua-relay` | Accepted audiences, comma-separated. |
| `CUA_RELAY_ACCOUNT_CLAIM` | `sub` | Token claim naming the account. |
| `CUA_RELAY_STATE_FILE` | in memory | Machine directory (JSON). |
| `CUA_RELAY_SIGNING_KEY_FILE` | fresh per process | Ed25519 assertion key, created on first use. |
| `CUA_RELAY_DEVICES_FILE` | next to the state file | Devices and audit log (`machines.json` gives `machines.devices.json`; in memory without a state file). Its audit hash-chain key (S9) is generated alongside it, 0600, at `<devices file>.audit-key`; back this file up with the devices file, or the chain from before a restore stops verifying (a fresh key generates rather than fail closed, since a lost audit key must not mean a lost relay). |
| `CUA_RELAY_MAX_MACHINES_PER_ACCOUNT` | `32` | Machines one account may register. |
| `CUA_RELAY_DEVICE_ENROLLMENT` | `on` | Require enrolled client devices (`on` / `off`). |
| `CUA_RELAY_DEVICE_TTL_DAYS` | `30` | Days an enrollment lasts without use (each device session renews it). |
| `CUA_RELAY_DEVICE_GRACE_DAYS` | `0` (ended) | Days unenrolled devices keep access after enrollment is first on. Set this only for a time-boxed rollout window (S3): a stolen account token alone reaches every machine while it is open. |
| `CUA_RELAY_DEVICE_BOOTSTRAP_MAX_AUTH_AGE_SECS` | `600` | Max sign-in age that enrolls a device without an approval. |
| `CUA_RELAY_DEVICE_REQUIRE_APPROVAL` | `false` | `true`: strict enrollment: a fresh sign-in enrolls only an account's first device, a re-key of a known machine, or (verified email) a sign-in proving MFA; other new devices need an approval from an enrolled device. `phones`: the same rule for phone-class devices only (token issued to a `CUA_RELAY_PHONE_CLIENTS` client, or a phone platform), every other device keeps the default; needs `CUA_RELAY_PUSH=on` and a non-empty `CUA_RELAY_PHONE_CLIENTS`, else the relay refuses to start. Any other value is an error. |
| `CUA_RELAY_PUSH` | `off` | Push notifications for the phone app (`on` needs account mode). On also turns on the phone viewer rules below. Off, `/v1/push/...` answers 404 and the relay behaves exactly as before. See "The phone app". |
| `CUA_RELAY_PUSH_FILE` | next to the state file | The account's registered phones and preferences (`machines.json` gives `machines.push.json`; in memory without a state file). Holds device tokens: 0600, back it up with the devices file. |
| `CUA_PUSH_APNS_KEY_FILE`, `CUA_PUSH_APNS_KEY_ID`, `CUA_PUSH_APNS_TEAM_ID`, `CUA_PUSH_APNS_TOPIC` | unset | The APNs authentication key (a `.p8` file from the Apple Developer account), its ids and the app's bundle id (default `ai.cua.spaces.mobile`). With push on and no key, phones register and alerts wait; a key file that is named but unreadable, or without its ids, stops the relay at start. Never put the key in the image or the repository: mount it as a secret. |
| `CUA_PUSH_LOG_FILE` | unset | A local run without an Apple key: write each alert (no token) to this JSON-lines file instead of sending it. Never set in production. |
| `CUA_RELAY_TOKENS` | unset | Static mode: comma-separated registration tokens. |
| `CUA_RELAY_TOKEN_FILE` | unset | Static mode: one token per line (`#` comments). |
| `CUA_RELAY_MAX_MACHINES_PER_TOKEN` | `16` | Static mode: machines per registration token. |
| `CUA_RELAY_JOIN_RATE_PER_SEC` | `250` | Computer joins admitted per second once the burst is used. A restart brings every computer back at once: the rest get `503` with a jittered `Retry-After`, which the join client honours. `0` turns the limit off. |
| `CUA_RELAY_JOIN_BURST` | `500` | Computer joins admitted at once. |
| `CUA_RELAY_ALLOW_ANONYMOUS_CLIENTS` | off | Static mode: also forward requests without a spacesd credential. |
| `CUA_RELAY_HOST_DOMAIN` | unset | Host mode: `<machine-id>.<domain>` routes to that machine, and path mode (`/m/<id>/…`) refuses on the main origin (S6; see "Machine origin" below). |
| `CUA_RELAY_ADMIN_TOKEN` | unset | Enables `GET /relay/v1/machines`. |
| `CUA_RELAY_MAX_STREAMS` | `256` | Concurrent client streams per machine (app-level check; S7). |
| `CUA_RELAY_MACHINE_MAX_STREAMS` | `256` | yamux's own per-machine stream cap (S7); keep `>=` the value above. |
| `CUA_RELAY_MACHINE_WINDOW_BYTES` | `67108864` (64 MiB) | yamux's per-machine connection receive window (S7); must be `>=` `machine-max-streams * 256 KiB`. |
| `CUA_RELAY_MAX_GLOBAL_STREAMS` | `4096` | Concurrent client streams across every machine at once: the relay-wide memory budget (S7). Tune to the pod's memory limit. |
| `CUA_RELAY_IDLE_SECS` | `90` | Seconds without a heartbeat before a machine is dropped (min 5). |
| `CUA_RELAY_SHUTDOWN_GRACE_SECS` | `10` | Seconds open requests get after SIGTERM / SIGINT. Machine tunnels and mesh connections are closed at once, so they reconnect to the next relay. |
| `CUA_RELAY_HEALTH_LISTEN` | unset | Probe address (`0.0.0.0:8081`) answering `GET /healthz` from start, standby included. Point the Kubernetes probes here for rolling deploys. |
| `CUA_RELAY_WEB_CLIENT_ID` | unset | OIDC client id of the browser sign-in for share links (public client + PKCE; redirect URI `<CUA_RELAY_PUBLIC_URL>/v1/web/callback`). Needs account mode, `CUA_RELAY_PUBLIC_URL` and `CUA_RELAY_HOST_DOMAIN`. Unset: no browser sign-in, `/v1/web/*` is 404 and a browser without a token gets 401. |
| `CUA_RELAY_WEB_CLIENT_SECRET` | unset | Client secret, for a confidential web client. |
| `CUA_RELAY_TRUSTED_PROXIES` | empty | Comma-separated CIDRs of the reverse proxies in front of the relay (`10.42.0.0/16`). `X-Forwarded-For` is believed only when the connecting socket is inside this set, and then the client is the right-most entry that is not itself a trusted proxy. Empty: the header is ignored and the connecting address is the client (the right setting for a relay reached directly). Used for the public-share rate limit. |
| `CUA_RELAY_SHARE_PUBLIC_RPS_PER_IP` | `20` | Sustained requests per second one client address may send to public share links. |
| `CUA_RELAY_SHARE_PUBLIC_BURST_PER_IP` | `60` | Burst for the same. |
| `CUA_RELAY_SHARE_PUBLIC_RPS_PER_MACHINE` | `200` | Sustained requests per second one machine's public links take from everyone together. |
| `CUA_RELAY_SHARE_PUBLIC_BURST_PER_MACHINE` | `600` | Burst for the same. |
| `CUA_RELAY_RECONNECT_GRACE_SECS` | `15` | For this long after start, a request for a known machine that is not connected yet waits for it to reconnect instead of failing. `0`: fail at once. |
| `CUA_RELAY_STUN_ONLY` | off | Serve only STUN (`CUA_RELAY_MESH_STUN_LISTEN`) and `/healthz` (`CUA_RELAY_LISTEN`): run STUN as its own pod, so relay pods can overlap during a deploy. |
| `CUA_RELAY_MESH` | `off` | The mesh coordinator (`on` / `off`). Off unless set; clients then use the HTTPS path. |
| `CUA_RELAY_MESH_RELAY` | `on` | Relay mesh ciphertext between peers that cannot reach each other directly. |
| `CUA_RELAY_MESH_STUN_LISTEN` | unset | UDP address to answer STUN on (`0.0.0.0:3478`). |
| `CUA_RELAY_MESH_STUN` | the public host on the STUN port | STUN servers advertised to nodes (`host:port`, comma-separated). |
| `CUA_RELAY_MESH_FILE` | `<state>.mesh.json` | The mesh registry. |
| `CUA_RELAY_ANALYTICS_KEY_FILE` | `<state file>.analytics-key` | 32-byte secret behind the PostHog distinct id and the short tags log lines carry for an account (HMAC-SHA256; created 0600 on first use). Back it up with the state files; a new key only changes the ids. |
| `CUA_RELAY_AUDIT_RETENTION_DAYS` | `400` | Days audit entries (devices and teams) are kept, on top of the per-account count limit. `0`: count limit only. Expired entries leave the file at start and hourly; the hash chain of what is left still verifies. |
| `CUA_RELAY_MESH_ENDPOINT_RETENTION_DAYS` | `7` | A mesh node offline longer than this loses its stored endpoints (its key registration stays). `0`: never. |
| `CUA_RELAY_STATE_RESERVE_PERCENT` | `15` | Pool writes leave at least this share of the state volume free (device, session, directory and mesh writes may use it). |
| `CUA_RELAY_STATE_RESERVE_MIB` | `256` | ... and at least this many MiB. |
| `CUA_RELAY_POOL_ACCOUNT_QUOTA_MIB` | `512` | Most one account's pool keeps on the state volume (log, terminal job keys, snapshot). |
| `RUST_LOG` | `info` | Log filter. |

Connected machines live in the process, and the directory is a local file:
run one replica per hostname.

**Deploys without downtime.** One relay at a time owns the state directory:
it holds an exclusive `flock` on `<state dir>/.relay.lock` (the kernel drops
it when the process exits, so it is never stale). A second relay on the same
volume binds its ports and waits in standby: its main port queues
connections (the listen backlog) instead of refusing them, and
`CUA_RELAY_HEALTH_LISTEN` answers probes. When the first relay gets SIGTERM
it closes every machine tunnel and mesh connection, lets open requests
finish and exits; the second takes the lock, loads the state and serves the
queued requests and reconnecting machines. On Kubernetes: a rolling update
with `maxSurge: 1, maxUnavailable: 0`, the new pod on the old pod's node (a
ReadWriteOnce volume mounts on one node, in any number of its pods), probes
on the health port and a short `preStop` sleep so the ingress stops sending
to the old pod before it stops serving. See `deploy/relay.k8s.yaml` and
`src/handover.rs`. Switch an existing Recreate deployment over in two steps:
first deploy this version with Recreate (the old version takes no lock),
then the rolling update.

## State and monitoring

Everything the relay keeps lives next to the state file, on one volume:

| Store | File | How it is written |
|---|---|---|
| Machine directory | `<state>` | Compact JSON, rewritten (streamed, temp file + rename) only when a record really changes. A failed write refuses the change (`500 storage`); memory keeps the old record. |
| Devices | `<state>.devices.json` | Compact JSON, devices only, rewritten (streamed) per device change. A failed write refuses the change. |
| Audit log | `<devices>.audit.jsonl` | One fsynced line per event; the last 5000 per account and nothing older than `CUA_RELAY_AUDIT_RETENTION_DAYS` in memory; rewritten when trimmed lines pass 50 000 and at once when entries expire by age. A failed write drops the event (counted). |
| Device sessions | `<devices>.sessions.jsonl` | One fsynced line per session opened or ended; rewritten with the live ones when it holds 4096 lines more than twice their number. Best effort: a lost line only costs a client a new session (an ended device's sessions are refused regardless). |
| Mesh registry | `<state>.mesh.json` | Compact JSON, one coalesced write per burst of changes (off the async workers), retried with backoff on failure. A new node waits for the write that covers it; a pairing is confirmed only once saved. Soft state: nodes register again on reconnect. Endpoint addresses nodes report are never written to it. |
| Pools | `<state>.pool/<account>/` | `log.jsonl` and `terminal.txt` appended (fsynced) per append; `lease.json`, `keys.json`, `snapshot.bin` replaced whole. A failed append is cut back (no torn line) and answered `500 storage`. |
| Pairing codes | `<state>.pairing.json` | Compact JSON, rewritten (streamed) per mint, spend or revoke. A failed write refuses the mint or spend (`500`); the code stays as it was. |
| Space settings | `<state>.spaces.json` | Compact JSON of each account's default Space and fallback, rewritten (streamed) per change. A failed write refuses the change (`500`). |
| Pending deletions | `<state>.pending-deletions.json` | Compact JSON of removed computers' Spaces still to delete on them, rewritten (streamed) per removal and ack; records older than 90 days are dropped. A failed write refuses the removal (`500`, nothing removed). |
| Grants | `<state>.grants.json` | Compact JSON of who reached each machine and whose access was revoked, rewritten (streamed) on a change. A failed write is not rolled back (a revoke holds in memory, so old tickets stay refused) and is retried with the next authorization. |

Every replaced file goes through a temp file (`.<name>.<uuid>.tmp`); startup
removes the ones a killed process left (counted in
`cua_relay_state_temp_files_removed_total`). A torn last line of an
append-only file is dropped at startup, from the file too, so later appends
never follow a broken line.

**Pool limits.** One account's pool keeps at most
`CUA_RELAY_POOL_ACCOUNT_QUOTA_MIB` (512 MiB) and its log at most 128 MiB or
200 000 entries between snapshots; past either an append is refused with
`507` and `code` `quota` or `snapshot_needed` (`retryable: true`), and the
coordinator writes a snapshot and retries. Every pool write must also leave
the volume's reserve free (`CUA_RELAY_STATE_RESERVE_PERCENT`, 15%, and
`CUA_RELAY_STATE_RESERVE_MIB`, 256 MiB, measured with `statvfs`); past it
appends are refused with `507 disk_reserve` and `Retry-After`, while
devices, sign-ins and the directory keep writing into the reserve. A
snapshot may use half the reserve, so a full log can always be compacted.
Bodies: appends, requests and answers up to 64 MiB (a submit of 100 000
jobs), snapshots up to 64 MiB, keyrings 4 MiB, the rest 64 KiB.

**Endpoints.**

- `GET /healthz`: liveness, `204`. Never touches the disk. Point the
  Kubernetes liveness probe here.
- `GET /readyz`: the state volume. Writes, fsyncs and removes a 4 KiB probe
  file in the state directory (at most once a second), and reports free
  space (`disk.free_percent`, `disk.low_space` under 20%,
  `disk.pool_writes_refused`), each store's writes, failures and last error,
  `failing` (the stores whose last write failed within 5 minutes),
  `audit_events_dropped` and pool refusals. `503` when the probe fails or a
  store is failing. With one replica, do not make this the Kubernetes
  readiness probe (a full volume would take the whole relay, mesh included,
  out of service); alert on it instead.
- `GET /metrics`: Prometheus text, no account data:
  `cua_relay_state_writes_total{store}`,
  `cua_relay_state_persist_failures_total{store}`,
  `cua_relay_state_store_ok{store}`, `cua_relay_audit_events_dropped_total`,
  `cua_relay_state_probe_ok`, `cua_relay_state_probe_failures_total`,
  `cua_relay_state_temp_files_removed_total`,
  `cua_relay_pool_writes_refused_total{reason}`,
  `cua_relay_state_volume_bytes`, `cua_relay_state_volume_available_bytes`,
  `cua_relay_state_pool_reserve_bytes`.
  `cua_relay_connections_total{kind,outcome}` counts connection attempts
  for an availability SLO: `kind="machine"` (a computer opening its tunnel;
  `outcome` is `ok`, `rejected` for a caller-side refusal or `error` for a
  relay-side failure) and `kind="client"` (a request or stream forwarded to
  a computer; `outcome` is `ok`, including the computer's own 4xx and 5xx
  answers, `rejected`, `offline` when the computer is not connected, or
  `error` for a relay-side failure). No other label values exist, and all
  seven series are present from process start.
- `GET /v1/info` keeps `state_writable` (false while a store is failing).

**Probe contract (`x-cua-relay-route`).** External probes match the
status code and this header, never the wording of a refusal: the text a
person reads changes with the relay's mode (phone support on answers an
unknown id as an account computer) and with copy edits. Every response of
`/healthz` (on every listener), `/readyz` and the client route
(`/m/<id>/...`, host mode, any path forwarded to a computer) carries one of
these values (`src/route_marker.rs`):

| Value | Meaning |
|---|---|
| `ok` | `/healthz` (204), or `/readyz` ready (200) |
| `not-ready` | `/readyz` 503 |
| `forwarded` | the computer answered; status and body are its own (a computer cannot set this header itself) |
| `refused-auth` | relay 401: no credential or one it does not accept |
| `refused-denied` | relay 403: not shared, stopped sharing, revoked ticket |
| `refused-limit` | relay 429 or 503: plan, rate or capacity limit |
| `refused` | any other relay 4xx |
| `offline` | the computer is not connected (502) |
| `error` | any other relay 5xx |
| `relay` | the relay answered itself without refusing (a redirect, a page it serves) |

gRPC refusals (HTTP 200 with `grpc-status`) carry the marker of the
relay's own status. An anonymous `GET /m/00000000-0000-0000-0000-000000000000/healthz`
answers `401` with `refused-auth` in every mode (phone support on or off,
admin token set or not, account sign-in on or off), and an edge or
load-balancer error page carries no marker at all. The values are fixed:
`tests/probe_contract.rs` fails if one changes or disappears. Changing them
means changing the probes in trycua/cloud in the same release.

**Log lines to alert on** (stable message and `alert` field):
`state persist failed` (`alert=state_persist_failed`, with `store` and
`error`; at most one line per store per second, `since_last_line` counts
the rest), `state volume write probe failed` (`alert=state_probe_failed`),
`audit events dropped` (`alert=audit_event_dropped`, at 1, 2, 4, 8, ...
drops), `pool write refused` (`alert=pool_write_refused`, at powers of two).

**Alert on:**

1. `/readyz` not `200` (the probe fails or a store is failing), for 2
   minutes.
2. Free space under 20%: `/readyz` `disk.free_percent < 20`, or
   `cua_relay_state_volume_available_bytes / cua_relay_state_volume_bytes < 0.2`.
3. Any persist failure: `increase(cua_relay_state_persist_failures_total[10m]) > 0`,
   `increase(cua_relay_audit_events_dropped_total[10m]) > 0`, or any
   `state persist failed` log line.

`examples/state_stress.rs` reproduces the failure class on a small
size-limited volume (a disk image or tmpfs, never a disk that matters):
audit churn with `kill -9` mid-write, pool appends into the reserve, a
volume filled to the last byte, and a mesh session across a killed relay
restarted on a full volume.

Pool refusals for space (`cua_relay_pool_writes_refused_total{reason="disk_reserve"}`)
mean the volume is at its reserve: grow it before the reserve runs out.

## State, backups and retention

What the relay keeps, for how long, and what backs it up.

**Logs and analytics name accounts by a keyed tag.** A log line never carries
an account id (Keycloak `sub`) or an email: it carries an 8-hex tag, and the
PostHog distinct id is `relay_` plus 16 hex, both HMAC-SHA256 under the
relay's own key (`CUA_RELAY_ANALYTICS_KEY_FILE`, see the table above). Knowing
an account id does not tell you its tag or analytics id. Account erasure
returns the analytics id the relay used, so the caller can delete the person
in PostHog.

| Data | Kept |
|---|---|
| Machine directory (`<state>`) | Until the machine is removed or the account erased. A machine that is offline stays registered. |
| Devices (`<state>.devices.json`) | Until removed or the account erased. A revoked device stays, as a revoked key, so it cannot enroll again. |
| Device audit log | Last 5000 per account and at most `CUA_RELAY_AUDIT_RETENTION_DAYS` (400). |
| Team audit log | Last 5000 per team and at most the same days. |
| Device sessions | Until they expire (1 hour by default). |
| Pairing codes | Until 24 hours after they expire or are used. |
| Grants (who reached what) | About 24 hours (the ticket lifetime plus 10 minutes). |
| Mesh registry (`<state>.mesh.json`) | Node keys until 30 days offline. Endpoint addresses are never written to disk and are dropped from memory after `CUA_RELAY_MESH_ENDPOINT_RETENTION_DAYS` (7) offline. |
| Usage totals (hosted usage file) | The current month. |
| Pools | Per-account quota (`CUA_RELAY_POOL_ACCOUNT_QUOTA_MIB`) until the account is erased. |

**Backups.** The relay does not back itself up; whatever backs up its state
directory (one volume: every file above, the signing key, the audit key and
the analytics key) holds all of this, for as long as the backup lives.
Restoring a backup brings back data the relay has since dropped.

- *Self-hosted:* yours to decide. Back up the whole state directory together
  (the audit and analytics keys go with the files they protect), and apply your
  own retention to the backups.
- *Hosted relay (relay.cua.ai):* its state is one encrypted volume. What
  is backed up, and for how long, is on the "What the relay can see" docs page
  (`docs/content/docs/spaces/guides/relay-security.mdx`).

## The phone app

Two additions serve the Cua Spaces phone app. Both are off until
`CUA_RELAY_PUSH=on`: a relay without it behaves exactly as before, including
how it treats the viewer.

**Push (`push_api`, the `cua-push` crate).** With `CUA_RELAY_PUSH=on`, an
enrolled phone registers with the token its operating system gave the app
(`PUT /v1/push/registrations/{device_id}`: `platform`, `provider`, `token`,
`app_id`, `env`, `categories` (`needs_you`, `done`), `locale`; `DELETE` to
remove it) and the account keeps preferences (`GET`/`PUT /v1/push/preferences`,
`GET /v1/push`). A daemon of the account reports an event
(`POST /v1/push/events` with the account's token: the event's kind, the Space
and entry it is about, the agent's short name and a unique id), and the relay
sends an alert to that account's phones only. The alert is built here from a
fixed phrase per kind: no message text, tool output or file content ever
reaches a phone. An event id is sent once however often it is reported, a
token the provider says is gone is dropped, and an account may report 120
events a minute. A phone registers only itself (the path's device id must be
the one its device session is of). Without an APNs key, registrations are kept
and alerts wait.

**The web viewer from a phone's web view.** A web view can set headers on a
page's first request but not on what the page then loads. In account mode with
`CUA_RELAY_PUSH=on`, the relay therefore handles the viewer like this
(`viewer.rs`). With it off, an anonymous viewer page request is forwarded to the
machine and any well-formed ticket header is forwarded, as before:

- A request **without any credential** for `GET`/`HEAD` `/m/<id>/viewer`,
  `/viewer/` or `/viewer/<file>` is answered **by the relay itself** from the
  viewer embedded in its build (`cua-spacesd-html5`, about 280 KB of static
  files). It never reaches a machine, so a machine cannot serve its own page
  under the relay's origin, and an unknown id, an offline machine, a machine
  that is someone else's and one that stopped sharing all get the same bytes
  with no lookup to time. The path is percent-decoded once and checked against
  an allowlist (`A-Za-z0-9._~-` and `/`; no `%` left, backslash, NUL or control
  character, `.` or `..` segment, or `//`). The responses carry
  `Content-Security-Policy: <the viewer's policy>; sandbox allow-scripts` (no
  `allow-same-origin`: the page runs in an opaque origin and cannot read this
  origin's storage or call its API as the user), `X-Content-Type-Options:
  nosniff`, `Referrer-Policy: no-referrer`, `Cache-Control: no-store` on the
  page. Each address may make 10 such requests a second (burst 40, then 429).
- Because the sandboxed page is cross-origin to the relay, the relay answers
  the CORS preflight (`OPTIONS`) of the viewer's own calls (`/cua.env.v1.*`,
  `/files...`) itself, the same for every id.
- The viewer's calls carry its machine-minted ticket in
  `x-cua-env-authorization: Bearer v1.<claims>.<signature>`. The relay forwards
  such a call only when the ticket is a **viewer** ticket that has **not
  expired** (read from its claims; a viewer ticket names no machine, so a
  ticket from another machine fails its signature there), is at most 2 KiB,
  and the call is one the viewer makes (`POST /cua.env.v1.*`, `/files...`,
  `GET /media`). Revocation is checked as for the media socket's ticket
  (`grants`). An address whose tickets machines refuse 30 times in a minute is
  answered 429 for the rest of the minute. A machine's own static token never
  has that shape and is never forwarded.
- With the account's own bearer, nothing changes: the relay's assertion
  replaces every client credential header, and a viewer page a machine serves
  to such a caller gets the same sandbox policy.

What the sandbox takes from the viewer in a web view (the page works: it loads,
reaches the relay with CORS and sends its ticket; checked in headless Chrome):
`sessionStorage` (the ticket stays in the URL fragment, so a reload still
works), the microphone uplink and the async clipboard (their permission
policies name `self`, which an opaque origin is not), and the folder picker
for file sync. If those are needed, serve the viewer from its own origin
instead (a separate static host that holds nothing), rather than weakening the
sandbox.

The phone mints the ticket with its own authenticated call
(`SystemService.CreateViewerTicket`, a machine that shares its desktop) and
opens `<relay>/m/<id>/viewer/#ticket=<ticket>`.

### A phone is a smaller kind of device

With `CUA_RELAY_PUSH=on` the relay treats a device enrolled from the phone app
as a **phone**: a device that reports `ios` or `android`, or whose sign-in
token was issued to one of the phone client ids (`CUA_RELAY_PHONE_CLIENTS`,
default `cua-spaces-mobile`). The mark is kept with the device and never
removed: registering the same key again as a computer does not undo it.

- **Short list.** A phone's calls reach a machine only if they are on the list
  in `phone.rs`: the capabilities and health probes, a view-only viewer ticket,
  the screen (`ListDisplays`, `Screenshot`) and the Space thread
  (`CallThreadTool`). Everything else (processes, files, tunnels, the
  clipboard, creating Spaces, every plain HTTP route) is refused here with
  `a phone cannot make this call`, before it is forwarded. The machine checks
  the same list again.
- **A shared desktop is a Space.** `Pointer` and `Keyboard` are forwarded, and
  the machine answers them only on a desktop it shares (`share_desktop`) and
  only when the account's role may touch it (the owner's own phone and an
  editor may; a view-only share may not). There is no second consent for a
  phone: sharing the desktop is the consent, and stopping the share ends a
  phone's running picture within a second. The computer's own services (files,
  processes, tunnels, the driver, shutdown) are never a phone's.
- **The assertion says so.** The machine gets the phone's device id and kind
  (`dev`, `devk`) in the relay-signed assertion, from the verified device
  session, never from anything the phone sends.
- **A phone cannot enroll a computer.** Approving a device from a phone is
  refused unless the device is a phone too.
- **Revoking a phone ends its sessions at once**, like any device.
- **Without `CUA_RELAY_PUSH=on`** a phone's device session is refused
  (`this relay does not serve phones`): a phone is served only through these
  rules.
- **No enumeration.** With phone support on, an id nobody registered is
  answered exactly like a machine that is not shared with the caller (401
  without a valid token, 403 with one).
- **Pin the clients.** `CUA_RELAY_OIDC_AUTHORIZED_PARTIES` (comma separated)
  restricts account tokens to the listed OAuth clients (`azp`). Unset accepts
  any client of the issuer, as before. With phone support on, set it to the
  clients meant for this relay (the CLI, the apps and `cua-spaces-mobile`).
- **The push key.** `CUA_PUSH_APNS_KEY_FILE` must be readable by its owner
  only (`0600` or `0400`); the relay refuses to start with a looser one and
  never logs the key.

### Deployment requirements for phone support

The relay refuses to start with `CUA_RELAY_PUSH=on` unless the two settings that make it safe are said (only a
`--public-url` that is plain `http://`, a local run, is let off; https or no address at all is public):
- **`CUA_RELAY_OIDC_AUTHORIZED_PARTIES`**: the OAuth clients allowed to sign in (the CLI, the desktop app and
  `cua-spaces-mobile`). A token for any other client of the issuer is refused. `CUA_RELAY_PHONE_CLIENTS` names which
  of them are phones; its default is the client id the phone app signs in with (`cua-spaces-mobile`), so the identity
  provider's client for the phone app must have that id.
- **`CUA_RELAY_TRUSTED_PROXIES`**: the ingress addresses, and the ingress must overwrite (not append to)
  `X-Forwarded-For`; every rate limit keys on the caller's address. When clients reach the relay directly, say so
  with `none`.

Also required, and not something the relay can check: account mode with device enrollment, grace period 0, strict
approval, `/metrics` and `/readyz` details off the public listener (they need the admin bearer; block them at the
ingress too), and an identity provider where only those clients carry the relay's audience.

- **Slow-header clients.** With phone support on, the relay drops a client that takes more than 10 seconds to send a
  request's headers and refuses headers over 32 KiB (it serves through its own bounded listener).
- **Computers that predate phones.** A computer says (`x-cua-phone-support`) that it serves phones when it connects.
  One that does not is sent nothing of a phone's; the phone is told to update that computer. For the ones that do,
  a phone's `CreateViewerTicket` is rewritten before it is forwarded: view only, five minutes at most, nothing else.
- **Devices from before.** A device enrolled as an iPhone, iPad or Android before phones were a class is a phone.
- **A phone and the account's control plane.** Resharing, stopping or starting sharing, removing or confirming a
  computer, and minting a pairing code from a phone need a sign-in made in the last ten minutes. A phone never takes
  part in the pool (`/v1/pool/*`) or reports events.
- **Kill switches.** One phone: revoke it (ends its sessions, tickets, streams and alerts). Every phone of an
  account: `POST /v1/devices/revoke-phones` (from a computer, or a phone with a recent sign-in), also a button in the
  app. Every phone of the relay, keeping the hardening: `CUA_RELAY_PHONES=off`. One computer: `cua-spacesd phone off`,
  or stop sharing its desktop: a running picture stops within a second.

### What changes in the relay, and the deploy checklist

What changes, all behind `CUA_RELAY_PUSH=on`: the routes `/v1/push/...`; the
unauthenticated viewer path above (a new, bounded, relay-served answer for
`/m/<id>/viewer...` and the preflight of the viewer's calls); header tickets
are forwarded only when they are live viewer tickets on viewer calls (before,
any syntactically valid ticket header was forwarded on any path and method);
the relay now links the viewer's static files (`cua-spacesd-html5`).
Everything else is unchanged.

Checklist, on a pre-production relay first:
1. Deploy with `CUA_RELAY_PUSH` unset and check nothing moved: an anonymous
   `GET /m/<id>/viewer/` is answered by the machine, as before.
   Then set `CUA_RELAY_PUSH=on` (account mode): `GET /m/<any id>/viewer/`
   returns 200 and `content-security-policy` ending in `sandbox allow-scripts`;
   the same for an id that does not exist; `GET /m/<id>/files` is still 401.
2. With a real machine that shares its desktop and a phone's web view (or a
   desktop browser), open `<relay>/m/<id>/viewer/#ticket=<ticket>`: the screen
   appears and touch works; microphone and clipboard are expected to be off.
3. Watch `cua_relay_connections_total{kind="client",outcome="rejected"}` and the
   auth log for `ticket_revoked`: no rise outside tests.
4. Add the APNs variables to the relay that has `CUA_RELAY_PUSH=on`; `GET /v1/push` with a
   phone's token says `"configured": true`; register a phone, post an approval
   in a Space, see the alert.
5. Production the same way. To roll back, unset `CUA_RELAY_PUSH` and deploy the
   previous image; nothing is stored that the old relay needs (`*.push.json` is
   a new file next to the state file).

## Run and test

```bash
# From libs/cua-spacesd
cargo run -p cua-relay -- --tokens dev-token           # static mode on :8080
cargo run -p cua-relay --example fake_oidc -- \
  --listen 127.0.0.1:9000 --issuer http://127.0.0.1:9000 --sub ada --email ada@example.com
CUA_RELAY_OIDC_ISSUER=http://127.0.0.1:9000 cargo run -p cua-relay   # account mode

cargo test --locked -p cua-relay
```

`examples/local-harness.rs` is the whole local setup for the phone app: a
fake issuer and a relay (account mode, device enrollment, push with a log
provider) on loopback ports, all state under one directory.
`scripts/mobile-harness.sh start` adds a machine's daemon in an isolated home
(no service is installed, nothing outside that directory is touched), and
`scripts/phone-probe.py` plays the phone against it (`scripts/mobile-e2e.sh`
runs the wire end to end). Never production: the issuer signs a token for
anyone who asks.

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

### Verify the relay image

The image build is reproducible: base images are pinned by digest, the Rust
toolchain matches `libs/cua/rust-toolchain.toml`, and the final stage runs no
package manager. To check that a published image matches a commit, check the
commit out and rebuild it with that commit's time as `SOURCE_DATE_EPOCH`
(Docker with BuildKit 0.13 or newer):

```bash
git checkout <commit>
libs/cua-spacesd/crates/cua-relay/scripts/check-reproducible.sh   # linux/amd64
# or one build, to compare against a digest you were given:
SOURCE_DATE_EPOCH=$(git log -1 --format=%ct) docker buildx build --platform linux/amd64 \
  -f libs/cua-spacesd/crates/cua-relay/Dockerfile \
  --build-arg SOURCE_DATE_EPOCH=$SOURCE_DATE_EPOCH \
  --output type=oci,dest=relay.tar,rewrite-timestamp=true libs
tar -xOf relay.tar index.json   # manifests[0].digest is the image digest
```

The script builds twice (the second without cache) for each variant, the
default build and `CUA_RELAY_HOSTED=1`, and fails if an image digest or the
`cua-relay` binary sha256 differs. The two variants differ from each other by
design; pass `--build-arg CUA_RELAY_HOSTED=1` to the single build above to
rebuild the hosted one. Digest equality assumes the same platform and tree.
CI runs the check on every change to the relay or its Dockerfile
(`ci-cua-relay-image.yml`).
