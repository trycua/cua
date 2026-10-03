# CUA-954: Expose Fleet device authentication through the public `cua-sandbox` API

## Context

`cua-sandbox` 0.1.17 supports Fleet-backed cloud sandboxes, but its public
configuration only supplies OAuth client credentials. `_FleetClient` resolves
`CUA_CLIENT_ID` and `CUA_CLIENT_SECRET` and always creates the generated
Cyclops client with `CyclopsClient.connect(...)`. This prevents a user who has
authenticated with the CUA CLI from using that user token to create and destroy
Fleet sandboxes without reaching into `_FleetClient` internals.

The generated Cyclops SDK already provides the needed public transport seam:
`AccessTokenProvider.get_access_token(force_refresh)`,
`CyclopsTokenProviderConfiguration`, and
`CyclopsClient.connect_with_access_token_provider(...)`. CUA-954 should expose
that path through `cua-sandbox` rather than asking callers to replace private
client fields or methods.

Repository evidence collected from `origin/main` at
`d614d8db689cd663f9e8d784ff03ac1bd388f506`:

- `libs/python/cua-sandbox/cua_sandbox/_config.py` stores only client-credential
  Fleet settings and `configure()` has no token-provider argument.
- `libs/python/cua-sandbox/cua_sandbox/transport/fleet_cloud.py` raises when
  client credentials are absent, then constructs `CyclopsConfiguration` and
  calls `CyclopsClient.connect(...)`.
- `libs/fleet/sdk-bindings/python/cyclops_sdk/_sdk.py` exposes the async
  `AccessTokenProvider` contract and
  `CyclopsClient.connect_with_access_token_provider(...)`.
- `libs/python/cua-sandbox/README.md` contains the current Fleet quickstart and
  documents only `CUA_CLIENT_ID`/`CUA_CLIENT_SECRET`.
- `libs/python/cua-sandbox/tests/test_fleet_cloud_client.py` is the focused unit
  test module for Fleet client construction, while integration tests are
  collected from `tests/integration/sandbox_sdk` by the package pytest config.

## Implementation Plan

1. Extend the public Fleet configuration surface in
   `libs/python/cua-sandbox/cua_sandbox/_config.py`.
   - Add a typed, optional token-provider setting to `_Config` and expose it as
     a keyword argument on `cua_sandbox.configure(...)`.
   - Add a resolver for the configured provider so transport construction does
     not read or mutate private configuration fields.
   - Define and document deterministic authentication selection: a configured
     token provider takes precedence for Fleet; otherwise retain the existing
     client-ID/client-secret and environment-variable behavior unchanged.
   - Keep the provider contract compatible with the generated SDK: an async
     `get_access_token(force_refresh: bool) -> str` method. Do not add a second
     device flow in `cua-sandbox`; callers own refresh behavior through the
     provider.

2. Make `libs/python/cua-sandbox/cua_sandbox/transport/fleet_cloud.py` select
   the generated SDK connection matching the public configuration.
   - When a token provider is configured, build
     `CyclopsTokenProviderConfiguration` with the existing Fleet base URL and
     polling limits, then call
     `CyclopsClient.connect_with_access_token_provider(...)` with the shared
     HTTP client.
   - When no provider is configured, preserve the current
     `CyclopsConfiguration` plus `CyclopsCredentials` connection path and its
     existing missing-credentials error.
   - Keep ownership and cleanup of `CyclopsHttpClient` identical in both paths
     so creating, claiming, deleting, and closing Fleet resources continue to
     have one lifecycle.
   - Do not expose `_FleetClient`, `_client`, or generated-SDK implementation
     fields as a new public API.

3. Update the Fleet tutorial in `libs/python/cua-sandbox/README.md` to use the
   supported public API.
   - Retain client credentials as the service-account/automation option.
   - Add the CLI-authenticated-user workflow: obtain a refresh-capable provider
     from the CLI-authentication integration, pass it with
     `cua.configure(token_provider=...)`, and create an ephemeral Fleet
     sandbox without `CUA_CLIENT_ID` or `CUA_CLIENT_SECRET`.
   - Remove any private `_FleetClient` imports, assignments to private client
     fields, or replacement of private client methods from the tutorial. The
     sample must use only documented `cua-sandbox` configuration and sandbox
     APIs.
   - State that token refresh is delegated to the provider and that the
     existing client-credentials flow remains supported for non-interactive
     workloads.

4. Add focused unit coverage in
   `libs/python/cua-sandbox/tests/test_config.py` and
   `libs/python/cua-sandbox/tests/test_fleet_cloud_client.py`.
   - Verify `configure(token_provider=...)` is retrievable through the public
     resolver and does not regress current credential/environment precedence
     when no provider is supplied.
   - Use a minimal async fake provider plus mocked generated-SDK constructors
     to assert that the provider path uses
     `CyclopsTokenProviderConfiguration` and
     `connect_with_access_token_provider(...)`, not the credential constructor.
   - Verify the credentials path and its missing-credentials failure remain
     covered, and verify both paths close the same HTTP client.

5. Add an opt-in Fleet integration test under
   `tests/integration/sandbox_sdk` for the public token-provider path.
   - Gate it on the existing CLI-authenticated credential source and Fleet test
     environment so normal local/PR runs skip rather than attempting cloud
     provisioning.
   - Construct the provider through the new public API only; do not import or
     patch `_FleetClient`.
   - Provision a small registry-backed Fleet sandbox, execute a lightweight
     readiness command, and ensure context-manager teardown destroys its claim
     and pool. Include cleanup that runs after assertion failures to avoid
     leaked Fleet resources.
   - Configure the test process without `CUA_CLIENT_ID` or
     `CUA_CLIENT_SECRET`, proving a CLI-authenticated user can complete the
     create/destroy lifecycle via the supported token-provider route.

## Validation

1. Run the focused package tests for configuration and Fleet client
   construction from `libs/python/cua-sandbox`.
2. Run the package formatter/linter used by the Python workspace for the files
   changed above.
3. In a Fleet-enabled integration environment, run the new token-provider test
   with CLI credentials present and `CUA_CLIENT_ID`/`CUA_CLIENT_SECRET` absent.
   Confirm provisioning succeeds and the pool/claim are deleted afterward.
4. Manually review the README example to confirm it imports no private Fleet
   symbols and assigns no private fields or replacement methods.

## Non-Goals

- Changing the legacy API-key cloud backend or its base-URL behavior.
- Replacing the CUA CLI authentication flow or implementing device flow inside
  `cua-sandbox`.
- Changing generated Cyclops SDK bindings; this work consumes the existing
  token-provider API.
