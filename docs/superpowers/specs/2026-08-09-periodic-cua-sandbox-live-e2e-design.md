# Periodic Cua Sandbox Live Fleet E2E Design

## Problem

The `cua-sandbox` repository has focused unit and contract coverage for Fleet
resource generation, but ordinary pull request CI does not prove that the SDK
can provision a real sandbox through `https://run.cua.ai`, reach the guest
computer-server, and remove the resources it created.

The repository previously had
`.github/workflows/periodic-test-linux-cloud-vm.yml`. It ran a Linux cloud VM
smoke every 15 minutes, used a repository secret, and notified Alertmanager on
failure. That workflow was removed on July 10, 2026 as a legacy cloud VM test.
The replacement should retain its fast detection and operational alerting while
testing the current Fleet-backed public SDK contract.

## Goals

- Exercise Fleet-backed `Sandbox.ephemeral()` against live production
  infrastructure every 15 minutes.
- Test both the current repository `main` source and the latest published
  `cua-sandbox` package.
- Run the source lane immediately after relevant changes merge to `main`.
- Verify guest access, generated Fleet port configuration, and owned-namespace
  cleanup.
- Alert through Alertmanager with enough lane and version context to triage a
  failure.
- Prevent failed or superseded runs from accumulating live infrastructure.

## Non-Goals

- The workflow is not a required pull request check.
- There is no separate nightly or broad lifecycle suite.
- The workflow does not test snapshots, forks, Android, Windows, or multiple
  regions.
- The live smoke does not test `server_port=5000` until a pinned image serves
  the computer-server `/cmd` API on that port. Existing contract tests remain
  authoritative for non-default port propagation.
- The workflow does not use the legacy `/api/keys` API or a namespace-scoped
  key.

## Trigger And Lane Model

`.github/workflows/periodic-cua-sandbox-live.yml` has three triggers:

1. `schedule` uses `7/15 * * * *`, preserving the `:07`, `:22`, `:37`, and
   `:52` cadence.
2. `push` is limited to `main` and these paths: `libs/python/cua-sandbox/**`,
   `libs/python/cua-fleet/**`,
   `.github/workflows/periodic-cua-sandbox-live.yml`, and
   `.github/scripts/tests/test_periodic_cua_sandbox_live.py`.
3. `workflow_dispatch` accepts `both`, `main-source`, or `published-package`,
   plus manual-only `force_failure`.

The preparation script emits a JSON matrix: every `push` selects only
`main-source`; every `schedule` selects both lanes; manual runs select their
requested lane or both. The workflow contract executes this extracted shell
script in `bash` with a temporary `GITHUB_OUTPUT` and parses the emitted JSON,
so a push-to-both mutation or ignored manual selection fails CI.

The two lanes use `fail-fast: false` and this concurrency contract:

```yaml
concurrency:
  group: periodic-cua-sandbox-live-${{ github.event_name }}-${{ matrix.lane }}
  cancel-in-progress: ${{ github.event_name == 'schedule' }}
```

The event-and-lane grouping means a new schedule can cancel only an older
schedule for the same lane. Push and manual runs use distinct groups and are
allowed to finish.

## Installation Isolation

`main-source` installs `libs/python/cua-sandbox` editable;
`published-package` installs the current published `cua-sandbox` wheel. Both
lanes then run `Prepare isolated live test suite`, which copies only
`tests/__init__.py` and `tests/live/*.py` into a temporary
`CUA_LIVE_E2E_TEST_ROOT`, then runs:

```bash
PYTHONPATH="$CUA_LIVE_E2E_TEST_ROOT" python -m pytest -q -s \
  "$CUA_LIVE_E2E_TEST_ROOT/tests/live/test_fleet_ephemeral.py"
```

The checkout package root is not on that test path. Therefore the source lane
uses its editable install while the published lane imports `cua_sandbox` from
site-packages. The live summary records the resolved `cua_sandbox` module
origin alongside package versions and source SHA.

## Authentication And Secrets

Fleet authentication uses only masked GitHub Actions secrets `CUA_CLIENT_ID`
and `CUA_CLIENT_SECRET`, with `CUA_FLEET_BASE_URL=https://run.cua.ai`. Before
checkout, installation, or pytest, `Check Fleet OAuth credentials` exits with
an error if either value is empty. This prevents production monitoring from
passing through the live test's credential-free pytest skip.

No credential value, access token, or authorization header is written to logs
or uploaded artifacts. The workflow does not create temporary user keys.

## Live Scenario

Place the stable scenario in
`libs/python/cua-sandbox/tests/live/test_fleet_ephemeral.py`. The test is
opt-in and skips unless the OAuth environment is configured, keeping ordinary
local and pull request test runs credential-free.

Each lane generates a DNS-safe name containing the lane, GitHub run ID, and run
attempt. That name is used for the SDK-created namespace and makes resources
attributable to one workflow execution.

Provision with the exact certified image:

```text
296062593712.dkr.ecr.us-west-2.amazonaws.com/desktop-workspace-duo@sha256:5b9cb82f482834f7541901b87be956e7544d0db13fabc0b372cbc5eca5a74180
```

Use the public SDK with:

- `cpu=4`
- `memory_mb=4096`
- `server_port=8000`
- `time_to_start=900`
- `request_timeout=60`
- `telemetry_enabled=False`

The pinned digest isolates SDK and Fleet regressions from unrelated image-tag
movement. Image publication health belongs in a separate image workflow.

## Assertions

While the sandbox context is active, the test verifies:

1. the sandbox has a non-empty name and is bound to the expected namespace;
2. the generated Fleet template exposes the named `server` Service with
   `targetPort=8000`;
3. the generated readiness probe uses `tcpSocket.port=8000`;
4. `sandbox.screen.size()` returns exactly `1024x768`;
5. a screenshot starts with the PNG signature and has a nontrivial payload;
6. `sandbox.shell.run("uname -s")` succeeds and returns `Linux`.

The direct Fleet template assertions are deliberate. Guest screen and shell
success prove connectivity, while the template checks prove that the public
`server_port` input reached live infrastructure rather than succeeding through
an unrelated default or stale resource.

## Cleanup And Leak Detection

`Sandbox.ephemeral()` remains the primary cleanup mechanism. After its context
exits, the test polls Fleet until the owned namespace is absent. Namespace
absence is the terminal cleanup assertion because deletion also removes the
template, pool, claim, sandbox, VMI, pod, and Service resources beneath it.

Cleanup follows a diagnostic-only, leak-safe sequence:

1. preserve the original scenario exception, if any;
2. wait a bounded period for automatic namespace deletion;
3. if the namespace remains, collect a sanitized resource inventory; and
4. fail the test explicitly as an SDK cleanup regression.

The test and workflow never call Fleet namespace deletion directly. Fleet
namespace deletion is name-only and can race with namespace recreation, so
`Sandbox.ephemeral()` remains the sole cleanup authority. Cleanup errors are
reported alongside the primary exception.

## Diagnostics And Artifacts

The live test writes a sanitized JSON summary containing lane, source SHA,
installed package versions, the resolved `cua_sandbox` module origin, namespace
and resource names, assertion timing, screen and shell observations, automatic
cleanup results, and remaining resources on a leak. On failure only, the
workflow uploads this summary and related diagnostics with short retention.

A manual `force_failure` first runs `Write controlled failure diagnostics` to
create a sanitized `summary.json` containing `ControlledFailure`, then exits
nonzero so the failure-only artifact path is itself certifiable. The versions
step uses `tee -a "$GITHUB_OUTPUT"` to log and publish `sandbox` and `fleet`
outputs for the Alertmanager payload; it never reads its own step outputs.

Cleanup remains diagnostic-only and without explicit deletion: the workflow
never calls `cleanup_namespace` or `delete_namespace`.

## Alertmanager

Each lane has an `if: failure()` notification step posting to
`https://am.cua.ai/api/v2/alerts`. Use an alert such as
`PeriodicCuaSandboxLiveE2EFailed` with labels:

- `severity=critical`
- `service=cua-sandbox`
- `job=periodic-cua-sandbox-live`
- `lane=main-source|published-package`

Annotations include the failed GitHub Actions run, the source SHA or installed
package version, the pinned image digest, and a link to the workflow dashboard.
Do not include credentials or raw authorization failures that may contain
headers.

The lane label lets Alertmanager group repeated failures without combining a
source regression with a published-package or shared-infrastructure failure.

## Workflow Contract Coverage

The repository-side contract parses the workflow with `yaml.BaseLoader` and
asserts triggers, path filters, the executed preparation matrix, checkout ref,
event-and-lane concurrency, credential preflight, copied-suite isolation,
version output handling, controlled-failure diagnostics, failure-only artifacts,
Alertmanager labels, full-SHA action pins, and absence of explicit deletion.
Scripts CI installs `pyyaml` and runs this contract when the workflow changes.

## Rollout

1. Add the live test, workflow, and workflow contract test with the schedule
   temporarily disabled or guarded.
2. Manually dispatch `main-source` and verify assertions, diagnostics, and
   complete namespace cleanup.
3. Manually dispatch `published-package` and verify the installed release plus
   cleanup behavior.
4. Exercise the Alertmanager payload without exposing secrets, then resolve the
   test alert.
5. Enable the `7/15 * * * *` schedule.
6. Observe at least two consecutive scheduled runs for both lanes before
   considering rollout complete.

## Success Criteria

- Both lanes pass against live Fleet infrastructure with the pinned Duo image.
- A relevant merge to `main` receives an immediate source-lane result.
- Every scheduled interval starts both lanes; a newer schedule can cancel only
  an older scheduled run of the same lane, while push and manual runs finish.
- Port, screen, screenshot, and shell assertions all exercise the public SDK.
- Normal and failing runs leave no test namespace behind.
- A forced failure produces one actionable, lane-specific Alertmanager alert
  and sanitized failure artifacts.
