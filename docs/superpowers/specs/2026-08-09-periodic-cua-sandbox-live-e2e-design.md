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

Add `.github/workflows/periodic-cua-sandbox-live.yml` with three triggers:

1. `schedule` using `7/15 * * * *`, preserving the previous offset cadence at
   `:07`, `:22`, `:37`, and `:52` each hour.
2. `push` to `main`, path-filtered to the Fleet and sandbox SDK implementation,
   the live test, and the workflow itself.
3. `workflow_dispatch` with a lane input supporting `both`, `main-source`, and
   `published-package`, plus a manual-only `force_failure` boolean used to
   certify the Alertmanager route without provisioning a sandbox.

A small preparation job produces the lane matrix:

- scheduled runs execute `main-source` and `published-package`;
- relevant pushes to `main` execute only `main-source`, because the matching
  package may not have reached PyPI yet;
- manual runs execute the selected lane or both lanes.

The two lanes run independently with `fail-fast: false`. Concurrency is scoped
per lane so a slow or stuck scheduled run cannot overlap the next run for the
same lane. A superseded scheduled run may be cancelled, but a push or manual
run should finish so it retains useful certification evidence.

## Installation Isolation

Both lanes run the same stable test scenario but use separate Python
environments:

- `main-source` checks out the triggering `main` SHA, installs the local
  `libs/python/cua-sandbox` package, and resolves its declared `cua-fleet`
  wheel dependency from the configured package indexes.
- `published-package` checks out the repository only for the test harness, then
  installs the latest `cua-sandbox` release from PyPI without editable local
  SDK packages.

The workflow logs the exact Git SHA for `main-source` and the installed
`cua-sandbox` plus `cua-fleet` versions for `published-package`. This makes it
clear whether a failure belongs to unreleased source, a published artifact, or
shared live infrastructure.

The published lane is intentionally a public-package consumer. The test must
not import repository-private helpers or rely on editable-package behavior.

## Authentication And Secrets

Fleet authentication uses OAuth client credentials exposed only as masked
GitHub Actions secrets:

- `CUA_CLIENT_ID`
- `CUA_CLIENT_SECRET`

The backing user key is unrestricted (`scope=[]`) so the test may create and
delete its uniquely named namespace across all namespaces owned by the CI user.
The workflow sets `CUA_FLEET_BASE_URL=https://run.cua.ai` explicitly.

No credential value, access token, or authorization header is written to logs
or uploaded artifacts. The test does not create a temporary user key during
each run; key rotation is an operational concern outside this workflow.

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

Cleanup follows a fail-detecting, leak-safe sequence:

1. preserve the original scenario exception, if any;
2. wait a bounded period for automatic namespace deletion;
3. if the namespace remains, collect a sanitized resource inventory;
4. call Fleet `delete_namespace()` as emergency cleanup;
5. fail the test explicitly as an SDK cleanup regression, even if emergency
   cleanup succeeds.

The emergency path must not hide the original failure. Cleanup errors are
reported alongside the primary exception, and the workflow still attempts a
final best-effort namespace deletion in an `always()` step.

## Diagnostics And Artifacts

The test writes a sanitized JSON summary containing:

- lane, source SHA, and installed package versions;
- namespace and resource names;
- provisioning, readiness, assertion, and cleanup timings;
- observed screen dimensions;
- shell exit status;
- cleanup result and any remaining resource kinds.

On failure only, upload the JSON summary, relevant sanitized logs, resource
inventory, and final screenshot with short retention. Successful scheduled runs
keep their evidence in the GitHub Actions log without producing recurring
artifacts.

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

Add a repository-side workflow contract test under `.github/scripts/tests/`.
It verifies that the workflow retains:

- the 15-minute offset schedule;
- scheduled execution of both lanes;
- source-only execution for relevant pushes to `main`;
- manual lane selection;
- pinned image digest and explicit port `8000`;
- bounded job timeout and per-lane concurrency;
- automatic plus emergency cleanup;
- failure-only diagnostics upload;
- lane-labeled Alertmanager notification;
- commit-SHA-pinned GitHub Actions dependencies.

This test runs in the existing scripts CI and prevents a workflow refactor from
silently weakening its operational contract.

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
- Every scheduled interval starts both lanes without overlapping a previous run
  of the same lane.
- Port, screen, screenshot, and shell assertions all exercise the public SDK.
- Normal and failing runs leave no test namespace behind.
- A forced failure produces one actionable, lane-specific Alertmanager alert
  and sanitized failure artifacts.
