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
- Exercise claiming from persistent, pre-provisioned Fleet pools — both a warm
  pool and a scale-to-zero pool — on the same cadence.
- Test both the current repository `main` source and the latest published
  `cua-sandbox` package.
- Run the source lane immediately after relevant changes merge to `main`.
- Verify guest access, generated Fleet port configuration, and claim-only
  cleanup with persistent reconciled resources.
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
3. `workflow_dispatch` accepts a lane (`both`, `main-source`, or
   `published-package`), a suite (`both`, `ephemeral`, or `pool`), plus
   manual-only `force_failure`.

Both jobs are guarded with `if: github.repository == 'trycua/cua'`. A fork
that syncs `main` or enables the schedule therefore never runs the live smoke,
never fails the credential preflight, and never posts a fork-originated
`PeriodicCuaSandboxLiveE2EFailed` alert to the public Alertmanager endpoint.

The preparation script emits a JSON lane-and-suite matrix: every `push`
selects only `main-source` with the `ephemeral` suite; every `schedule`
selects both lanes crossed with both suites; manual runs select their
requested lane and suite combinations. The workflow contract executes this
extracted shell script in `bash` with a temporary `GITHUB_OUTPUT` and parses
the emitted JSON, so a push-to-both mutation or ignored manual selection fails
CI.

The matrix jobs use `fail-fast: false` and this concurrency contract:

```yaml
concurrency:
  group: periodic-cua-sandbox-live-${{ github.event_name }}-${{ matrix.lane }}-${{ matrix.suite }}
  cancel-in-progress: ${{ github.event_name == 'schedule' }}
```

The event-lane-and-suite grouping means a new schedule can cancel only an
older schedule for the same lane and suite. Push and manual runs use distinct
groups and are allowed to finish.

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
origin alongside package versions and source SHA. After checkout, `Record checked out source SHA`
uses `git rev-parse HEAD` to publish `CUA_LIVE_E2E_SOURCE_SHA`; the live summary,
controlled-failure summary, and Alertmanager annotation use that exact value rather
than the triggering event SHA.

## Authentication And Secrets

Fleet authentication uses only masked GitHub Actions secrets `CUA_CLIENT_ID`
and `CUA_CLIENT_SECRET`, with `CUA_FLEET_BASE_URL=https://run.cua.ai`. Before
checkout, installation, or pytest, `Check Fleet OAuth credentials` exits with
an error if either value is empty. This prevents production monitoring from
passing through the live test's credential-free pytest skip.

The workflow uses step-scoped OAuth credentials only on `Check Fleet OAuth credentials`
and `Run live Fleet smoke`; checkout, setup, package installation, and copied-suite
preparation do not inherit either secret.

No credential value, access token, or authorization header is written to logs
or uploaded artifacts. The workflow does not create temporary user keys.

## Live Scenario

Place the stable scenario in
`libs/python/cua-sandbox/tests/live/test_fleet_ephemeral.py`. The test is
opt-in and skips unless the OAuth environment is configured, keeping ordinary
local and pull request test runs credential-free.

Scheduled and push lanes use reusable DNS-safe namespaces; manual ephemeral runs use a run-unique namespace. The workflow sets
`cua-live-${{ matrix.lane }}-${{ github.event_name == 'workflow_dispatch' && github.run_id || github.event_name }}`, yielding `schedule`, `push`, or `manual` namespaces.
Event-and-lane concurrency serializes each deterministic claim.

Provision with the exact certified image:

```text
public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04@sha256:80fff8a40f217a460cef7a60161adb3899eabd02c3451f18926b84d1f81b8da2
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

Fleet reconciliation intentionally preserves each dedicated namespace, pool, and
template. After `Sandbox.ephemeral()` exits, the monitor performs claim-only
cleanup verification after every provisioning attempt, including a failure before the context yields:

1. preserve the original scenario exception, if any;
2. poll until claims are absent;
3. collect a sanitized inventory; and
4. after successful provisioning, require exactly the pool/template named after the namespace and zero claims; pre-yield failures retain diagnostic inventory without imposing that invariant.

A remaining claim or any unexpected inventory is a diagnostic failure. The test
and workflow never explicitly delete a namespace, pool, or template: name-only
deletes can race with reconciliation. Cleanup errors are reported alongside the
primary exception.

## Persistent Pool Suite

The `pool` suite exercises the pre-provisioned pool consumer path that the
`ephemeral` suite cannot: pools that survive across runs and hand out claims.
`Run live Fleet pool smoke` executes
`libs/python/cua-sandbox/tests/live/test_fleet_pool_persistent.py` from the
same isolated copied suite. The suite runs on `schedule` and
`workflow_dispatch` only; pushes keep running only the `ephemeral` suite.

Each lane and event class owns two persistent pool namespaces, set by the
workflow as:

- `cua-live-pool-warm-${{ matrix.lane }}-${{ github.event_name == 'workflow_dispatch' && 'manual' || github.event_name }}`
- `cua-live-pool-cold-${{ matrix.lane }}-${{ github.event_name == 'workflow_dispatch' && 'manual' || github.event_name }}`

The warm mode keeps `replicas=1`, so a scheduled claim binds to an
already-running sandbox and releasing the claim recycles that sandbox back
into the pool. The cold mode expresses scale-to-zero with
`WarmPoolAutoscaling(min_pool_size=0, initial_pool_size=0, max_pool_size=1)`
because pool reconciliation rejects `replicas` below one; a claim then
cold-starts capacity through autoscaler demand.

Each run observes the pool first with `Pool.get`, recording
`pool_pre_existed` and the replica counts, then reconciles the pinned
configuration with `Pool.apply` using the same certified image digest,
`cpu=4`, and `memory_mb=4096`. Fleet evaluates authorization before
existence, so reading a pool in a namespace that has not been created yet
returns 403 rather than 404; the observe step treats both statuses as
not-pre-existed (`is_pool_missing_error`), mirroring the SDK's reconcile
semantics, and a genuine access denial still fails the run at `Pool.apply`
with the canonical `PoolAccessDenied` guidance. Reconciliation is idempotent: it bootstraps a
missing pool, heals drift, and never deletes. The claim uses
`Sandbox.ephemeral(pool=..., name=...)` with the claim name fixed to the
namespace, so an interrupted run's claim is adopted and released by the next
run. Exiting the context performs a claim-only release: the pool and template
must persist.

After release the monitor polls until claims are absent and requires the
reconciled inventory to contain exactly the named pool and template with zero
claims — the persistence mirror of the ephemeral suite's empty-inventory
invariant. Replica counts after release are recorded as telemetry only,
because warm-pool recycling and autoscaler decay are server-controlled. The
warm mode additionally asserts a claim-acquisition bound only when the pool
pre-existed with at least one ready replica; bootstrap runs record timing
without enforcing it.

## Diagnostics And Artifacts

The live test writes a sanitized JSON summary containing lane, source SHA,
installed package versions, the resolved `cua_sandbox` module origin, namespace
and resource names, assertion timing, screen and shell observations, claim-only cleanup results and persistent reconciled resources. On failure only, the
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
- `suite=ephemeral|pool`

Annotations include the failed GitHub Actions run, the source SHA or installed
package version, the pinned image digest, and a link to the workflow dashboard.
Do not include credentials or raw authorization failures that may contain
headers.

The lane and suite labels let Alertmanager group repeated failures without
combining a source regression with a published-package failure, or an
ephemeral provisioning failure with a persistent pool claim failure. Failure
artifacts are named per lane and suite so concurrent matrix jobs never collide
on upload.

## Workflow Contract Coverage

The repository-side contract parses the workflow with `yaml.BaseLoader` and
asserts triggers, path filters, the upstream-repository fork guard on both
jobs, the executed preparation matrix, checkout ref,
event-lane-and-suite concurrency, credential preflight, copied-suite isolation,
version output handling, controlled-failure diagnostics, failure-only artifacts,
Alertmanager labels, full-SHA action pins, and absence of explicit deletion.
Scripts CI installs `pyyaml` and runs this contract when the workflow changes.

## Rollout

1. Add the live test, workflow, and workflow contract test with the schedule
   temporarily disabled or guarded.
2. Manually dispatch `main-source` and verify assertions, diagnostics, and
   complete claim-only cleanup.
3. Manually dispatch `published-package` and verify the installed release plus
   cleanup behavior.
4. Manually dispatch the `pool` suite twice per lane: the first run
   bootstraps both persistent pools (`pool_pre_existed` false), the second
   proves a warm claim against pre-provisioned capacity and records cold
   scale-to-zero telemetry.
5. Exercise the Alertmanager payload without exposing secrets, then resolve the
   test alert.
6. Enable the `7/15 * * * *` schedule.
7. Observe at least two consecutive scheduled runs for both lanes before
   considering rollout complete.

## Success Criteria

- Both lanes pass against live Fleet infrastructure with the pinned Duo image.
- A relevant merge to `main` receives an immediate source-lane result.
- Every scheduled interval starts both lanes; a newer schedule can cancel only
  an older scheduled run of the same lane, while push and manual runs finish.
- Port, screen, screenshot, and shell assertions all exercise the public SDK.
- Normal runs retain only the named persistent pool/template with no claims; failed provisioning records read-only claim and inventory diagnostics without deleting resources.
- Pool-suite runs claim from and release back to persistent pools that survive
  the run, with the warm pool binding against pre-provisioned capacity.
- A forced failure produces one actionable, lane- and suite-specific
  Alertmanager alert and sanitized failure artifacts.

## Live Evidence Remediation

The monitor uses reusable namespaces for scheduled and push runs, while manual ephemeral runs use per-run namespaces to avoid stale ownership collisions.
Each lane has one DNS-safe namespace for each event class:

- `cua-live-<lane>-schedule` for scheduled runs
- `cua-live-<lane>-push` for pushes
- `cua-live-<lane>-<run-id>` for `workflow_dispatch`

The persistent pool suite adds `cua-live-pool-warm-<lane>-<event-class>` and
`cua-live-pool-cold-<lane>-<event-class>` namespaces whose pools and templates
deliberately outlive every run.

The event-lane-and-suite concurrency group serializes use of each
deterministic claim; only scheduled runs cancel an older scheduled run in the
same lane and suite. Fleet reconciliation preserves the namespace, pool, and
template, all named after the namespace. `Sandbox.ephemeral()` is verified
with claim-only cleanup: after exit the monitor polls until claims are absent,
records persistent reconciled resources, and requires exactly the named
pool/template with zero claims. It never explicitly deletes a namespace, pool,
or template.
