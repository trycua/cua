# cua-relay: hosted-relay features

The Cargo feature `hosted` (on for the hosted relay, relay.cua.ai) adds
allowance metering, Teams, account erasure and the `/internal/v1/*` routes
the hosted Spaces API calls. Its sources are `src/hosted.rs`,
`src/meter.rs`, `src/plan_source.rs`, `src/teams.rs`, `src/team_api.rs`,
`src/team_sharing.rs`, `src/org_access.rs` and `src/account_erase.rs`;
without the feature `src/free.rs` stands in for them (see the README).
A plain `cargo build` or Docker build is the self-hosted relay (everyone Free);
the hosted relay's image is built with `--build-arg CUA_RELAY_HOSTED=1`.

```sh
cargo build -p cua-relay --features hosted
cargo test -p cua-relay --features hosted
cargo test -p cua-spacesd-server --features relay-hosted --test mesh_metering
```

## Teams

- **Teams.** An account creates a team (`POST /v1/teams`) and is its
  owner; owners and admins invite by email (only that verified email joins)
  or by link (`<relay>/join/team/<code>`, use-limited, 7 days), remove
  members, add machines they own to the team, approve member devices and
  read the team's audit log (`GET /v1/teams/{id}/audit`); only the owner
  makes admins. A team machine is listed for every member (and their
  agents) with `team` set and reached as `shared` (an editor), but a
  member's device opens it only after an owner or admin approved that
  device for the team (the first attempt files the request; owners' and
  admins' own devices are approved on first use). Macs and macOS Spaces
  may be team machines like any other. Teams persist in
  `<state>.teams.json`. See `src/teams.rs` and `src/team_api.rs`.

## Allowance metering

In account mode the relay meters, per user and pooled per team, **live
sessions** (one user on one machine with a stream held open, such as a
desktop view, a terminal or an agent's process stream; it stays live 30 s
after its last stream closes) and **relay hours** (time with at least one
live session, per calendar month). Past an allowance nothing is refused:
proxied responses carry `x-cua-relay-allowance: exceeded; reasons=…;
upgrade=<url>` and `GET /v1/usage` (account token) reports
`soft_limit_reached` with the upgrade URL, so the client can show a prompt.
`GET /relay/v1/usage` (admin token) lists every user metered this month.
Enforcement is a separate, later change.

## Configuration

These flags exist only with the feature on (`cua-relay --help`).

| Env var | Default | Meaning |
|---|---|---|
| `CUA_RELAY_METERING` | `on` | Account mode: meter live sessions and relay hours against the plan allowances. Measure and prompt; plans in `CUA_RELAY_ENFORCE_PLANS` (default empty: off) are also refused at their live-session limit (see `CUA_RELAY_ENFORCE_PLANS`). |
| `CUA_RELAY_ALLOWANCES` | `free=2/20,pro=5/200,teams=5/200` | `plan=SESSIONS/HOURS`: live sessions at a time and relay hours per calendar month (UTC). For a team plan the numbers are per seat and pooled. Must include `free`. |
| `CUA_RELAY_ENFORCE_PLANS` | empty (off) | Plans whose caps are enforced (`free,pro`): a NEW session past the live-session or hours cap is refused on every path: a WebSocket upgrade or a long gRPC stream (process, terminal and watch RPCs, decided at stream open from the method path; list, health and other short calls are never refused) through the HTTPS proxy gets 429 (gRPC callers `ResourceExhausted`), an `x-cua-relay-error` of `plan_limit_sessions` or `plan_limit_hours`, used and limit headers and a plain sentence body; a mesh session a machine reports for a peer is refused by dropping that peer from the machine's network map for 30 s (the machine tears the session down, relayed packets stop; the held sessions of other peers are untouched). A machine the user already holds a session on is never refused and live sessions are never cut (in memory, one lock, plan from the cached entitlements). Other plans are measure and prompt only. |
| `CUA_RELAY_UPGRADE_URL` | `https://cua.ai/pricing` | Where the allowance prompt links (pricing page or a Payment Link). |
| `CUA_RELAY_USAGE_FILE` | next to the state file | This month's totals per user (`machines.json` gives `machines.usage.json`), written at most every 5 minutes. |
| `CUA_RELAY_PLANS_FILE` | unset (everyone Free) | Plans and teams, reloaded when it changes: `{"users": {"<sub or email>": "pro"}, "teams": {"<id>": {"plan": "teams", "seats": 5, "members": ["<sub or email>"]}}}`. |
| `CUA_RELAY_TEAMS_SOURCE` | `file` | Where team access comes from: `file` (the relay's `teams.json`), `shadow` (`teams.json` decides; Keycloak Organizations plus the Spaces API are evaluated beside it and every disagreement is logged under `cua_relay::teams_shadow` and counted in `cua_relay_teams_shadow_mismatches_total`) or `org` (organizations only; `/v1/teams/*` answers 410). See `src/org_access.rs`. |
| `CUA_RELAY_SPACES_API_URL` | unset | Spaces API base URL (in-cluster) for team access lookups. Required with `org`. |
| `CUA_RELAY_SPACES_API_TOKEN_URL` | the issuer's token endpoint | Where the relay gets its client-credentials token. |
| `CUA_RELAY_SPACES_API_CLIENT_ID` / `_CLIENT_SECRET` | `cua-relay-svc` / unset | The relay's service client. |
| `CUA_RELAY_ORG_CACHE_SECS` | `60` | How long a team access answer (yes or no) is cached. A miss with the API down refuses team access (personal machines are unaffected). |
| `CUA_RELAY_INTERNAL_TOKEN` | unset | Bearer token the Spaces API presents on `POST /internal/v1/membership-changed {org, sub?}`, which drops the cached answers and re-checks open sessions, and on `GET /internal/v1/records/{id}` -> `{id, owner_sub, platform, kind}` (404 `record_not_found`), which the API checks before binding a machine to an org. Under `org`, a bound record is reachable only while its owner is a member of the same org. Also guards `DELETE /internal/v1/accounts/{sub}` (see [Account erasure](#account-erasure)). Use at least 32 characters (`openssl rand -hex 32`). The relay refuses to start with a shorter token. No token while `CUA_RELAY_TEAMS_SOURCE=org` or `CUA_RELAY_TEAM_ONLY_SHARING=on` logs a loud error and the relay keeps serving; `CUA_RELAY_STRICT_INTERNAL_TOKEN=on` makes that a startup failure too. The token is never printed. Keep `/internal/` off the ingress. |
| `CUA_RELAY_STRICT_INTERNAL_TOKEN` | `off` | `on` refuses to start when the internal token is missing while the organization teams source or team-only sharing is on (and team-only sharing lacks the service client secret). Off logs an error and keeps serving, so an existing deployment stays up. A token shorter than 32 characters is always refused. |
| `CUA_RELAY_LIVE_SESSION_AFTER_SECS` | `10` | How long a stream must stay open before its session counts as live (short calls never do). |
| `CUA_RELAY_POSTHOG_KEY` | unset | PostHog project capture key: hourly `relay_usage` and `relay_allowance_soft_limit` events (hashed user ids, no emails). Unset: usage goes to the logs (`cua_relay::usage`) only. |
| `CUA_RELAY_POSTHOG_ENDPOINT` | `https://eu.i.posthog.com/batch/` | PostHog batch endpoint. |

## Internal routes and the ingress

`/internal/v1/*` is for the Spaces API, which calls the relay in-cluster, straight
to the Service. Two layers keep it off the public path:

- **Ingress deny.** Do not route `/internal/` from the ingress; the example
  manifest (`deploy/relay.k8s.yaml`) denies it explicitly. A catch-all host rule
  would otherwise forward it.
- **Relay refusal.** Any `/internal/v1/*` request carrying `X-Forwarded-For`,
  `X-Forwarded-Host`, `Forwarded` or `X-Real-IP` (that is, one that came through
  a proxy) is answered `404 not found` before the token is checked. In-cluster
  callers add none of these headers.

`CUA_RELAY_TEAM_ONLY_SHARING=on` also needs `CUA_RELAY_SPACES_API_CLIENT_SECRET`
(the relay's own service credentials for its calls to the Spaces API), checked
the same way as the token. Self-hosters should set `CUA_RELAY_STRICT_INTERNAL_TOKEN=on`.

## Account erasure

`DELETE /internal/v1/accounts/{sub}?email=<email>` erases or revokes everything
the relay holds for one account; the Spaces API calls it when a person deletes
their account. It needs the internal token (`CUA_RELAY_INTERNAL_TOKEN`, 404
when none is set, 401 when wrong) and must stay off the ingress. `email` is
optional and repeatable: every address shares or invites were made to. The relay adds the addresses it holds for the account itself.

It erases:

- the account's machines (record, live tunnel, team entries, mesh nodes), and
  its id and `email`s from other people's allowlists and viewers;
- live streams and outstanding tickets (cut; the tickets stay refused for their
  lifetime by a hash of the id, never the id);
- its devices (deleted, not revoked), device sessions, and its whole audit
  chain (other accounts' chains are separate and still verify), pairing codes;
- teams: it leaves every team, a team it solely owns is deleted, a team it owns
  with other members passes to the longest-standing admin (else member),
  invites addressed to its emails or created by it are revoked, and its name
  or email in a team's audit log becomes "deleted account";
- who-reached-what rows (`grants`), usage and queued analytics events, the
  cached plan and its snapshot, cached team-access answers, mesh pairings and
  the account's pool (memory and `<state>.pool/<hash>/`).

Idempotent: a repeat, or an unknown account, answers the same 200 with zero
counts. Resumable: a failed write answers `500` with `"complete": false` and a
`detail`; the same call again finishes (every store is rewritten on each call).
The answer is counts only, plus the PostHog distinct id the relay reports the
account under (`relay_` and 16 hex of SHA-256 of the id), so the caller can
delete that person in PostHog. The relay logs the account by a short hash only.

```json
{"machines":1,"shares_removed":1,"devices":1,"pairing_codes":1,"sessions_cut":2,
 "mesh_nodes":2,"usage":true,"plan_cache":false,"pool":true,"teams":4,"grants":3,
 "posthog_distinct_id":"relay_0123456789abcdef","complete":true}
```

`sessions_cut` is live client connections cut plus device sessions ended;
`usage`, `plan_cache` and `pool` say whether that store knew the account.

Other accounts' audit logs that name the erased account (its id or any of its
emails, for example an owner's "shared access" line) are rewritten to say
"deleted account", and that account's chain is re-chained from the first changed
entry under the relay's audit key, so it still verifies. Without `email`
parameters the relay finds the account's addresses in its own records (machine
owners, usage, teams, mesh nodes, pairing codes, grants) before erasing, and uses
them for share removal and this rewrite. On a retry after a partial failure those
records may already be gone, so pass `email` again.

Not covered: an operator's plans file (`CUA_RELAY_PLANS_FILE`) is operator-owned.

## State

| Store | File | How it is written |
|---|---|---|
| Usage | `<state>.usage.json` (`CUA_RELAY_USAGE_FILE`) | Compact JSON of this month's totals, streamed at most every 5 minutes and at shutdown. A failed write keeps the totals dirty and is retried with the next one. |
| Teams | `<state>.teams.json` | Compact JSON, rewritten (streamed) per team change. A failed write refuses the change (`500`); memory keeps the team as it was. |
