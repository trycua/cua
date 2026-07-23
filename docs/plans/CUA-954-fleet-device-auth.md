# CUA-954: Expose Fleet Device Authentication Through `cua-sandbox`

## Purpose

Enable an application that already owns a renewable user access-token source to
use Fleet through the public `cua-sandbox` API. This is a plan only: it does not
change authentication, routing, or documentation behavior.

The recommended public entry point is:

```python
cua.configure(access_token_provider=provider)
```

`provider` supplies an access token asynchronously and accepts a
`force_refresh: bool` request. The CLI will provide an adapter over its existing
device-login credentials; `cua-sandbox` will not read the CLI keyring itself.

## Refreshed Baseline And Evidence

- Historical inspection SHA: `ee15ae942cefe809fd97a565220eca9c6a295ac0`.
  This SHA is retained only to explain the prior tutorial observation; it is not
  the implementation or PR base.
- Refreshed PR base: `e3983eb27e47a87b6f6641651e00d05cb2eb8cfe`
  (`origin/main`, refreshed before creating this plan branch).
- `libs/python/cua-sandbox/cua_sandbox/_config.py` defines `_Config` at about
  line 15 and `configure()` at about line 27. It currently accepts legacy
  `api_key` settings and Fleet client-credential settings, but no access token
  or access-token-provider setting.
- `libs/python/cua-sandbox/cua_sandbox/sandbox.py` selects Fleet in
  `Sandbox._uses_fleet()` at about line 698 only when the call has no explicit
  `api_key` and `get_client_id()` plus `get_client_secret()` are available. A
  bearer token passed through `api_key` therefore stays on the legacy cloud
  transport today.
- `libs/python/cua-sandbox/cua_sandbox/transport/fleet_cloud.py` has
  `_FleetClient` at about line 42. Its constructor requires confidential
  `CUA_CLIENT_ID` and `CUA_CLIENT_SECRET`; `list_pools()` at about line 118
  issues a raw `/api/namespaces` request through `CyclopsHttpClient`.
- `FleetCloudTransport.connect()` at about line 206 creates a pool and claim
  for a newly provisioned sandbox. `_cleanup_resources()` at about line 280
  deletes the claim before deleting the pool, and `delete_sandbox()` follows
  the same resource order.
- The generated binding at
  `libs/fleet/sdk-bindings/python/cyclops_sdk/_sdk.py` exposes both
  `CyclopsClient.connect_with_access_token()` (about line 2700) and
  `CyclopsClient.connect_with_access_token_provider()` (about line 2721).
  Its callback interface is `AccessTokenProvider.get_access_token(force_refresh:
  bool) -> str` (about line 2439). The provider connection retries one
  control-plane `401` after requesting a forced refresh; the hand-written
  `/api/namespaces` path must implement equivalent behavior.
- `libs/python/cua-cli/cua_cli/auth/oidc.py:get_access_token()` (about line
  229) loads saved device credentials, refreshes them when near expiry, and
  persists the refresh result. `libs/python/cua-cli/cua_cli/auth/store.py`
  stores those credentials in the OS keyring. Tokens and refresh credentials
  must never be printed, exported, written to test output, or copied into an
  environment variable.
- `libs/fleet/backend/auth/authz.rego` recognizes `input.user.azp ==
  "cua-cli"` (about line 54). For an authenticated non-empty subject from that
  exact interactive client, it permits `/api/namespaces` and customer-facing
  `/api/k8s/{path...}` routes; infrastructure paths remain restricted. This
  must be proven against the live issuer, claims, and entitlement state before
  treating CLI device login as production-ready Fleet access.

## Documentation State

The historical SHA contains
`docs/content/docs/tutorials/your-first-cloud-sandbox.mdx`; it has no
`_FleetClient`, `monkeypatch`, or `monkey-patch` reference. On refreshed
`origin/main` (`e3983eb27e47a87b6f6641651e00d05cb2eb8cfe`), that tutorial file
is absent. The current tutorial index in
`docs/content/docs/tutorials/meta.json` contains local sandbox, Lume VM,
application-driving, and Cua Bench tutorials only. `docs/next.config.mjs`
still permanently redirects `/tutorials/your-first-cloud-sandbox` to
`/tutorials/your-first-local-sandbox`. The implementation documentation must
therefore add or update a current public SDK guide; it must not modify a
nonexistent tutorial or describe a private-client monkey-patch.

## Public API Options

### Option 1: Static Access Token

Add `cua.configure(access_token="...")` and call the SDK's static-token
connection.

- Advantages: minimal API and direct use of the generated SDK constructor.
- Costs: tokens expire, callers must invent refresh handling, and examples tend
  to encourage copied secrets.
- Decision: reject. Do not recommend copied static tokens for device auth.

### Option 2: Public Async Provider

Add `cua.configure(access_token_provider=provider)`, where the provider
implements `async get_access_token(force_refresh: bool) -> str`.

- Advantages: matches the generated SDK callback contract, supports refresh on
  a `401`, keeps credential ownership with the caller, and is usable outside
  the CLI.
- Costs: introduces a small public protocol/type and requires explicit retry
  support for raw control-plane requests.
- Decision: recommend this option.

### Option 3: Implicit CLI-Keyring Lookup In `cua-sandbox`

Have `cua-sandbox` import the CLI OIDC helper and silently read the OS keyring.

- Advantages: a short CLI integration path.
- Costs: couples the SDK to an optional CLI package and local credential-store
  behavior, surprises library consumers, complicates headless use, and makes
  credential boundaries unclear.
- Decision: reject. The CLI owns its keyring and supplies an explicit adapter.

## Recommended Behavior And Precedence

1. Add an exported async provider protocol or equivalent typed callable in
   `libs/python/cua-sandbox/cua_sandbox/_config.py`; save it in `_Config` and
   expose it through `cua.configure(access_token_provider=...)`.
2. Resolve Fleet routing in `Sandbox._uses_fleet()` and the transport factory
   with this order:

   ```text
   explicit per-call api_key -> legacy cloud
   explicit configured access_token_provider -> Fleet
   configured/environment client_id + client_secret -> Fleet
   otherwise -> existing legacy API-key behavior and errors
   ```

   An explicit `api_key` remains authoritative even if a provider and client
   credentials are configured. A provider intentionally takes precedence over
   ambient client credentials.
3. Update `_FleetClient` to accept the configured provider. For provider mode,
   construct `CyclopsClient` with the generated
   `connect_with_access_token_provider()` path. Preserve the client-credential
   constructor as Fleet's fallback.
4. Route `list_pools()` through a small authenticated request helper. It must
   obtain a token with `force_refresh=False`, send `Authorization: Bearer
   <token>`, and, only after one `401`, obtain a replacement with
   `force_refresh=True` and retry once. It must not log either token or its
   authorization header.
5. In `libs/python/cua-cli`, add an adapter that implements the sandbox
   provider contract by calling the existing OIDC client. Normal calls may use
   the existing valid cached token; forced calls must refresh even when the
   cached access token has more than the normal 60-second validity window.
   Persist refreshed credentials through the existing keyring store without
   exposing them.
6. Update CLI sandbox call sites only where the CLI should choose Fleet. Pass
   the adapter through public `cua.configure()` or the agreed public API; do
   not pass a CLI bearer token via `api_key` for Fleet operations. Preserve
   intentional legacy-cloud command paths that explicitly pass `api_key`.

## Sequential Implementation Plan

1. Define and export the provider type in
   `libs/python/cua-sandbox/cua_sandbox/_config.py`; extend `_Config`,
   `configure()`, and a getter without reading any CLI credential store.
2. Update `Sandbox._uses_fleet()` and each relevant transport-construction
   path in `libs/python/cua-sandbox/cua_sandbox/sandbox.py` so the precedence
   above is enforced consistently for create, connect, list, lookup, suspend,
   resume, restart, and delete operations.
3. Refactor `_FleetClient` in
   `libs/python/cua-sandbox/cua_sandbox/transport/fleet_cloud.py` to choose the
   generated provider connection when configured, preserve client-credential
   construction otherwise, and add the one-retry authenticated
   `CyclopsHttpClient` helper for `/api/namespaces`.
4. Keep provisioning and cleanup semantics unchanged: a newly created Fleet
   sandbox still creates a pool then claim, and cleanup still deletes claim
   then pool. Make errors retain the original provisioning failure while
   reporting cleanup failure as the cause, as the current transport does.
5. Add focused SDK tests in
   `libs/python/cua-sandbox/tests/test_config.py`,
   `libs/python/cua-sandbox/tests/test_fleet_cloud_client.py`, and
   `libs/python/cua-sandbox/tests/test_fleet_cloud_transport.py`. Cover
   provider selection, explicit-`api_key` legacy precedence, client-credential
   fallback, absent-configuration legacy behavior, initial bearer header,
   exactly one forced-refresh retry after `401`, and no retry for a non-`401`.
6. Add a CLI adapter and tests under
   `libs/python/cua-cli/cua_cli/auth/` and
   `libs/python/cua-cli/tests/auth/test_oidc.py`. Assert forced refresh calls
   the refresh flow and persists credentials; test token values with inert
   fixtures only and never print them. Update
   `libs/python/cua-cli/cua_cli/commands/sandbox.py` call sites only after the
   intended command-level Fleet behavior is agreed.
7. Add the opt-in live test
   `tests/integration/sandbox_sdk/test_fleet_cli_device_auth.py`. It must run
   only after an interactive `cua auth login` and only when
   `CUA_FLEET_TEST_IMAGE` is set to an approved image. Generate a unique name,
   create the Fleet sandbox through the public provider API, run a harmless
   command such as `printf fleet-device-auth-ok`, and in `finally` call the
   public sandbox delete API. Afterward, verify the named sandbox is absent.
   Do not read, print, snapshot, or export access or refresh credentials.
8. Add a current SDK guide or update the appropriate existing sandbox docs,
   documenting the provider protocol, precedence, login prerequisite for the
   CLI adapter, opt-in test image requirement, and credential-safety rules.
   Keep the existing old-tutorial redirect unless documentation owners request
   a deliberate redirect change.

## Validation Plan

Run focused unit tests from the repository root after implementation:

```bash
uv run python -m pytest \
  libs/python/cua-sandbox/tests/test_config.py \
  libs/python/cua-sandbox/tests/test_fleet_cloud_client.py \
  libs/python/cua-sandbox/tests/test_fleet_cloud_transport.py \
  libs/python/cua-cli/tests/auth/test_oidc.py -v
```

Run the opt-in credential-safe integration test only on an authorized machine
after interactive login and approval of the Fleet image:

```bash
cua auth login
CUA_FLEET_TEST_IMAGE='approved-image-reference' \
  uv run python -m pytest \
  tests/integration/sandbox_sdk/test_fleet_cli_device_auth.py -v
```

The integration test must skip with a clear non-secret message when
`CUA_FLEET_TEST_IMAGE` is not set. Review its captured output and logs to
confirm they contain neither `Authorization` headers nor token-shaped values.
Run the repository's applicable Markdown/docs formatter or docs build after
the documentation target is selected; do not add a new formatter solely for
this change.

## Risks And Decisions Required Before Shipping

- An approved Fleet image value for `CUA_FLEET_TEST_IMAGE` is not yet known.
  Keep the live test opt-in until an owner provides one.
- Live Keycloak issuer configuration, `azp`, subject, scopes/claims, and any
  Fleet entitlement must be verified with a real CLI device-login token by an
  authorized operator. The checked-in authorization policy is necessary but
  not sufficient evidence of production access.
- Confirm whether service-proxy `401` refresh is in scope. The generated Fleet
  SDK covers one control-plane retry; the raw `/api/namespaces` request must
  receive the same treatment. Service-proxy behavior may require a separate
  design and test if it does not flow through that SDK retry path.
- Preserve legacy compatibility: an explicit `api_key` must never silently
  switch to Fleet, and an application without provider or client credentials
  must retain today's legacy API-key errors and routing.
- Treat provider failures as authentication failures without including token
  material in exceptions, tracing, test assertion output, or telemetry.

## Acceptance Mapping

| Acceptance criterion | Planned evidence |
| --- | --- |
| Public SDK can use renewable device authentication | `cua.configure(access_token_provider=...)` plus provider-mode transport tests |
| CLI credentials remain private | CLI adapter reads/refreshes only through the OS keyring; tests and logs use no real credentials |
| Fleet routing is deterministic | Precedence tests for explicit `api_key`, provider, client credentials, and legacy fallback |
| Expired or rejected access token recovers once | SDK-provider behavior plus raw `/api/namespaces` initial request, forced refresh, and one-retry tests |
| Fleet lifecycle remains safe | Existing/new tests preserve pool-then-claim creation and claim-then-pool cleanup; E2E uses public delete in `finally` and verifies absence |
| Live coverage cannot consume an unapproved image | Integration test requires `CUA_FLEET_TEST_IMAGE` and skips otherwise |
| Documentation reflects current repository state | Current public SDK guide; no reference to a nonexistent old cloud tutorial or private `_FleetClient` patching |
