# CUA-954: Expose Fleet Device Authentication Through `cua-sandbox`

## Purpose

Enable short-lived interactive and prototyping Fleet sessions through the public
`cua-sandbox` API with a caller-supplied device access token. This is a plan
only: it does not change authentication, routing, or documentation behavior.

The recommended public entry point is:

```python
cua.configure(access_token="temporary-device-access-token")
```

Device tokens are deliberately short-lived. Expiration is a security property,
not a defect: Fleet should return an actionable re-authentication error rather
than silently refreshing a device token. Long-lived or unattended workloads
continue to use existing client ID/client secret authentication.

## Refreshed Baseline And Evidence

- Historical inspection SHA: `ee15ae942cefe809fd97a565220eca9c6a295ac0`.
  This SHA explains the prior tutorial observation only; it is not the PR base.
- Current PR base: `e3983eb27e47a87b6f6641651e00d05cb2eb8cfe`
  (`origin/main`, refreshed before the plan branch was created).
- `libs/python/cua-sandbox/cua_sandbox/_config.py` defines `_Config` at about
  line 15 and `configure()` at about line 27. It currently accepts legacy
  `api_key` settings and Fleet client-credential settings, but no device access
  token setting.
- `libs/python/cua-sandbox/cua_sandbox/sandbox.py` selects Fleet in
  `Sandbox._uses_fleet()` at about line 698 only when the call has no explicit
  `api_key` and both `get_client_id()` and `get_client_secret()` are available.
  A bearer token passed via `api_key` currently selects the legacy cloud
  transport, so device-token support needs an explicit public configuration.
- `libs/python/cua-sandbox/cua_sandbox/transport/fleet_cloud.py` has
  `_FleetClient` at about line 42. Its constructor requires confidential
  `CUA_CLIENT_ID` and `CUA_CLIENT_SECRET`; `list_pools()` at about line 118
  makes a raw `/api/namespaces` request through `CyclopsHttpClient`.
- `FleetCloudTransport.connect()` at about line 206 creates a pool and claim
  for a newly provisioned sandbox. `_cleanup_resources()` at about line 280
  deletes the claim before deleting the pool, and `delete_sandbox()` follows
  that same resource order.
- The generated binding at
  `libs/fleet/sdk-bindings/python/cyclops_sdk/_sdk.py` exposes
  `CyclopsClient.connect_with_access_token()` at about line 2700 and
  `CyclopsClient.connect_with_access_token_provider()` at about line 2721.
  CUA-954 will use the static access-token constructor only.
- `libs/python/cua-cli/cua_cli/auth/oidc.py:get_access_token()` at about line
  229 loads and refreshes device credentials from the CLI's OS keyring. Those
  credentials must never be printed, exported, copied into examples, or read
  automatically by `cua-sandbox`.
- `libs/fleet/backend/auth/authz.rego` recognizes `input.user.azp ==
  "cua-cli"` at about line 54. An authenticated non-empty subject from that
  exact interactive client is allowed `/api/namespaces` and customer-facing
  `/api/k8s/{path...}` routes, while infrastructure paths remain restricted.
  Live issuer, claims, and entitlement verification remains required before
  treating CLI device login as production-ready Fleet access.

## Documentation State

The historical SHA contains
`docs/content/docs/tutorials/your-first-cloud-sandbox.mdx`; it contains no
`_FleetClient`, `monkeypatch`, or `monkey-patch` reference. On current
`origin/main` (`e3983eb27e47a87b6f6641651e00d05cb2eb8cfe`), that tutorial file
is absent. `docs/content/docs/tutorials/meta.json` lists local sandbox, Lume
VM, application-driving, and Cua Bench tutorials only, while
`docs/next.config.mjs` permanently redirects
`/tutorials/your-first-cloud-sandbox` to
`/tutorials/your-first-local-sandbox`.

The implementation must add or update a current public SDK guide. Its example
must use supported public surfaces only, avoid private `_FleetClient` imports or
patches, and make clear that a device token is temporary. The exact supported
CLI token handoff mechanism must be confirmed before documenting it. If no
safe public mechanism exists, define one as an implementation/design requirement
without inventing a command that prints or persists secrets.

## Authentication Options

### Option 1: Static Device Access Token

Add `cua.configure(access_token="...")` and construct the generated Fleet
client with `CyclopsClient.connect_with_access_token()`.

- Advantages: small public surface, matches the generated SDK, makes the
  caller explicitly own a short-lived token, and is suitable for interactive
  device-auth prototyping.
- Costs: a token expires; callers must authenticate again when told to do so.
- Decision: recommend for short-lived interactive and prototyping sessions.

### Option 2: Async Refreshable Token Provider

Add a public provider that refreshes device credentials after a `401`.

- Advantages: avoids an explicit re-authentication interruption.
- Costs: expands the public API, obscures token expiration, couples CUA-954 to
  refresh lifecycle design, and risks spreading credential-store behavior.
- Decision: reject for CUA-954. Do not add a provider API, refresh behavior,
  provider tests, or a CLI keyring adapter.

### Option 3: Implicit CLI-Keyring Lookup

Have `cua-sandbox` import CLI authentication code and read the OS keyring.

- Advantages: a short integration path for the CLI.
- Costs: couples the SDK to an optional CLI package and local credential store,
  surprises library consumers, complicates headless use, and weakens the
  credential boundary.
- Decision: reject. `cua-sandbox` must not automatically read the CLI keyring.

## Recommended Behavior And Precedence

1. Add `access_token: Optional[str]` to the public `cua.configure()` contract
   and `_Config` in `libs/python/cua-sandbox/cua_sandbox/_config.py`. Treat it
   as an in-memory caller-supplied device token; do not add environment-based
   discovery, persistence, refresh, or CLI keyring access.
2. Resolve Fleet routing in `Sandbox._uses_fleet()` and the transport factory
   in this order:

   ```text
   explicit per-call api_key -> legacy cloud
   configured access_token -> Fleet static-token authentication
   configured/environment client_id + client_secret -> Fleet client credentials
   otherwise -> existing legacy API-key behavior and errors
   ```

   An explicit `api_key` remains authoritative even when `access_token` or
   client credentials are configured. A configured device token takes
   precedence over ambient client credentials for the interactive session.
3. Update `_FleetClient` to use
   `CyclopsClient.connect_with_access_token()` when `access_token` is present,
   retaining the client-credential constructor as the long-lived Fleet path.
4. Send `Authorization: Bearer <access_token>` for `list_pools()` and other raw
   `/api/namespaces` requests. Do not add forced-refresh-on-`401` semantics.
   A `401` or token-expiry response must fail with an actionable error that
   directs the user to authenticate again through the supported device-login
   flow, without echoing token material or authorization headers.
5. Do not add a CLI adapter. The CLI may integrate only through a confirmed
   supported handoff mechanism and public `cua-sandbox` configuration; it must
   not expose credentials in terminal output, files, environment exports, or
   logs. If that mechanism is absent, resolve it before promising CLI-driven
   Fleet device-auth examples.

## Sequential Implementation Plan

1. Extend `_Config`, `configure()`, and a token getter in
   `libs/python/cua-sandbox/cua_sandbox/_config.py` with an optional static
   `access_token`. Ensure its docstring identifies device tokens as temporary
   and does not imply background renewal.
2. Update `Sandbox._uses_fleet()` and relevant transport-construction paths in
   `libs/python/cua-sandbox/cua_sandbox/sandbox.py` to enforce the precedence
   above for create, connect, list, lookup, suspend, resume, restart, and
   delete operations.
3. Refactor `_FleetClient` in
   `libs/python/cua-sandbox/cua_sandbox/transport/fleet_cloud.py` to construct
   the generated client with `connect_with_access_token()` in token mode and
   preserve the existing client-credentials construction otherwise.
4. Add an authenticated raw-request helper for `/api/namespaces` that attaches
   the static bearer token. Translate an unauthorized or expired response into
   a credential-safe, actionable re-authentication error. Make exactly one
   request per operation; do not retry by refreshing the token.
5. Preserve lifecycle semantics: a newly created Fleet sandbox still creates a
   pool then claim, and cleanup still deletes claim then pool. Retain the
   existing behavior that preserves the provisioning failure if cleanup also
   fails.
6. Add focused SDK tests in
   `libs/python/cua-sandbox/tests/test_config.py`,
   `libs/python/cua-sandbox/tests/test_fleet_cloud_client.py`, and
   `libs/python/cua-sandbox/tests/test_fleet_cloud_transport.py`. Cover static
   token configuration, explicit-`api_key` legacy precedence, static-token
   precedence over client credentials, client-credential fallback, absent
   configuration legacy behavior, raw bearer authorization, and actionable
   no-retry handling of `401`/expiry.
7. Do not change CLI OIDC refresh behavior, add a CLI keyring adapter, or add
   CLI force-refresh tests for this issue. Separately confirm the supported,
   non-secret token handoff surface before wiring any optional CLI sandbox call
   sites.
8. Add the opt-in live test
   `tests/integration/sandbox_sdk/test_fleet_cli_device_auth.py`. Run it only
   after interactive `cua auth login`, only after the safe public handoff
   mechanism is confirmed, and only when `CUA_FLEET_TEST_IMAGE` names an
   approved image. Generate a unique name, configure the public static-token
   API, create the Fleet sandbox, run `printf fleet-device-auth-ok`, and in
   `finally` call the public delete API. Verify the named sandbox is absent
   afterward. Never read, print, snapshot, export, or persist access or refresh
   credentials.
9. Add a current SDK guide or update an appropriate existing sandbox document.
   Show only a supported safe way to obtain and supply the temporary token;
   state expiration and re-authentication behavior, recommend client
   credentials for long-lived/unattended sessions, and forbid persistence.
   Keep the old-tutorial redirect unless documentation owners explicitly change
   it.

## Validation Plan

Run focused unit tests from the repository root after implementation:

```bash
uv run python -m pytest \
  libs/python/cua-sandbox/tests/test_config.py \
  libs/python/cua-sandbox/tests/test_fleet_cloud_client.py \
  libs/python/cua-sandbox/tests/test_fleet_cloud_transport.py -v
```

Run the opt-in credential-safe integration test only on an authorized machine,
after interactive login, approval of the Fleet image, and confirmation of the
safe public token handoff design:

```bash
CUA_FLEET_TEST_IMAGE='approved-image-reference' \
  uv run python -m pytest \
  tests/integration/sandbox_sdk/test_fleet_cli_device_auth.py -v
```

The integration test must skip with a clear non-secret message when the image
or approved token handoff prerequisite is unavailable. Review test output and
logs to confirm they contain neither `Authorization` headers nor token-shaped
values. Exercise a deliberately expired or unauthorized inert test token in a
mocked unit test and assert one actionable re-authentication failure with no
retry. Run the repository's applicable Markdown/docs formatter or docs build
after the documentation target is selected; do not add a formatter for this
change.

## Risks And Decisions Required Before Shipping

- An approved Fleet image value for `CUA_FLEET_TEST_IMAGE` is not yet known;
  keep the live test opt-in until an owner provides one.
- Live Keycloak issuer configuration, `azp`, subject, scopes/claims, and Fleet
  entitlement must be verified with a real CLI device-login token by an
  authorized operator. The checked-in policy is necessary but not sufficient
  evidence of production access.
- The supported, credential-safe CLI device-token handoff path is not yet
  confirmed. Do not document a token-printing command, an environment export,
  or a persistence workflow. Resolve the public handoff design before adding a
  tutorial or CLI call-site integration.
- A device token is intentionally unsuitable for long-lived or unattended
  workloads. Documentation and errors must direct those users to client ID and
  client secret authentication instead of retrying or refreshing a token.
- Preserve legacy compatibility: an explicit `api_key` must never silently
  switch to Fleet, and an application without static token or client
  credentials must retain today's legacy API-key errors and routing.
- Treat unauthorized responses as credential-safe errors; do not include token
  material in exceptions, tracing, test assertion output, or telemetry.

## Acceptance Mapping

| Acceptance criterion | Planned evidence |
| --- | --- |
| Public SDK supports temporary device authentication | `cua.configure(access_token=...)` and static-token Fleet transport tests |
| Device-token lifetime is explicit | Docs and `401`/expiry tests direct users to authenticate again without retry or silent refresh |
| Long-lived workloads retain a safe path | Client ID/client secret remains Fleet fallback and is documented for unattended use |
| Fleet routing is deterministic | Precedence tests cover explicit `api_key`, static token, client credentials, and legacy fallback |
| Raw namespace requests authenticate correctly | Tests assert the static bearer header and a single actionable no-retry `401`/expiry failure |
| CLI and SDK credential boundaries remain private | No SDK keyring reads, no CLI adapter, no token output/export/persistence, and safe-handoff design is required before examples |
| Fleet lifecycle remains safe | Tests preserve pool-then-claim creation and claim-then-pool cleanup; E2E deletes publicly in `finally` and verifies absence |
| Live coverage cannot consume an unapproved image | Integration test requires `CUA_FLEET_TEST_IMAGE` and skips otherwise |
| Documentation reflects current repository state | Current public SDK guide avoids nonexistent old tutorial and private `_FleetClient` patching |
