# CI

Every pull request must pass one required check, **CI: gate** (`ci-gate`). The gate waits for the other
pull request workflows and passes when they pass. This page explains which of those workflows it waits for,
and the two checks in front of them: the quick stage and, for agent pull requests, the `local-ci` sign-off.

## Run the checks locally first: `bin/ci`

`bin/ci` runs, on your machine, the CI checks that your change can break, and prints the command that fixes
each failure. Run it before you push. It catches in minutes what otherwise costs a full CI round:

- rustfmt, and clippy `-D warnings` on the crates you changed and the crates that depend on them;
- each workspace's Windows CI lane, cross-compiled with `cargo xwin` on the same packages (read from the
  workflow), which catches code that only Unix uses. A C dependency that cannot be cross-built on your
  machine is reported as a warning, and every crate that does not need it is still checked;
- lockfiles that no longer match their manifests, including the nested workspaces under `examples/` and `tests/`;
- stale generated files: the cua SDK and Cua Driver UniFFI bindings, the Electron and SwiftUI app bindings,
  the Spaces contract manifest and control fixtures, and the Cua Driver contract;
- stale generated reference docs, docs code blocks with no disposition, and image refs outside the catalog;
- Python isort, black and ruff; TypeScript typecheck, tests and the design lint for the web packages you touched;
- actionlint for workflows, SPDX headers, and the unit tests of the crates you changed.
- Prettier on the Cua Spaces app files you changed, with each app's own `.prettierrc.yaml` (double quotes).

`CI: Quick` runs `bin/ci --quick` on every pull request, so these checks have no workflow of their own: SPDX
headers, lockfiles, actionlint, the larger-runner ban, image refs, Python lint, rustfmt and the docs image
refs (the last also runs nightly in `ci-check-docs-image-refs.yml`). On the runner a missing tool fails them
(`CUA_CI_REQUIRE_TOOLS=1`) instead of skipping them, and `bin/ci --verify-sync` fails if `ci-quick.yml`
stops running `bin/ci --quick`. Do not add a workflow that repeats one of them.

```bash
bin/ci                 # the checks your change affects (compares with the merge base with origin/main)
bin/ci --quick         # only checks that build nothing: seconds
bin/ci --fix           # apply the safe fixers (formatters, lockfiles, generators), then check again
bin/ci --full          # every check, whatever you changed
bin/ci --keep-target   # keep the cargo target dir (.bin-ci-target) between runs
bin/ci --list          # show the plan and the exact commands
bin/ci --only rust-fmt,lockfiles
```

### Windows

`bin/ci` runs on Windows from PowerShell, cmd or Git Bash: `python bin\ci --quick` (Python 3.10 or later;
`python3` is not needed). The checks are bash, so it runs them with **Git for Windows' bash**, found next
to `git` (set `CUA_CI_BASH` to another `bash.exe`). It never uses `bash` from `PATH`: there that is usually
`C:\Windows\System32\bash.exe`, which is WSL, and would run the checks inside a Linux distribution.
It runs in UTF-8 mode and gives the checks a `python3`. Cargo checks need the MSVC toolchain; on a shared PC
use `--quick`. The sign-off is the same (`python bin\ci --signoff`; its description ends with `(win32)`).

### Disk safety

Heavy checks (cargo test, clippy, build, cross-target) run one at a time, one crate per cargo call, with
`CARGO_INCREMENTAL=0`, no debuginfo and sccache when installed, in a per-run target dir that is deleted on exit.

| Variable | Default | Effect |
| --- | --- | --- |
| `CUA_CI_MIN_FREE_GB` | 40 | below this much free disk, heavy checks are skipped ("CI will run these") and the cheap checks still run |
| `CUA_CI_MAX_TARGET_GB` | 15 | a background check every 15 s stops cargo if the target dir grows past this |
| `CUA_CI_MIN_FREE_FLOOR_GB` | 25 | the same check stops cargo if free disk falls under this |

When the watcher stops cargo, `bin/ci` exits 3 with a "rerun in CI" hint. On shared hosts use `bin/ci --quick`
and leave heavy checks to CI or a VM.

The commands are the ones the workflows run. Each check names the workflow text it mirrors, and
`bin/ci --verify-sync` (run by **CI: Test Scripts**) fails when a workflow stops running it. When you change
how a workflow runs a check, change `bin/ci` in the same pull request.

A check whose tool is missing is reported as skipped, never as passed, with the install command. `bin/ci`
uses the pinned Rust toolchain (`rust-toolchain.toml`) through rustup even when another `cargo` comes first on
your `PATH`, and installs a web package's `node_modules` with its frozen lockfile when a check needs them
(`--no-install` skips those checks instead). Logs are kept under `.git/bin-ci/logs/`.

What it cannot catch: failures that only happen on a real Windows or Linux host (sockets, permissions,
installers), the end-to-end and image lanes, and flakes. CI still runs those.

### Pre-push hook

```bash
bin/ci --install-hook    # runs bin/ci before every git push from this clone and its worktrees
bin/ci --uninstall-hook
CUA_SKIP_LOCAL_CI=1 git push   # skip it once
```

Agent machines install the hook; for everyone else it is opt-in. The hook goes in the clone's
`hooks/pre-push`. An existing pre-push hook is kept and still runs first. If you set a global
`core.hooksPath`, your global pre-push hook must chain to the repository hook; the installer tells you
whether it does. Prefer `CUA_SKIP_LOCAL_CI=1` to `git push --no-verify`, which also skips every other
pre-push hook. `CUA_LOCAL_CI_ARGS="--quick"` changes what the hook runs.

### Local sign-off: `bin/ci --signoff`

After a green run on a clean tree, `bin/ci --signoff` sets the `local-ci` commit status on `HEAD` through
`gh api`. If the commit is not on GitHub yet, push and run it again: it reuses the green run. It refuses to
sign off a run that failed or skipped a check. Sign off again after every new commit.

`ci-gate` requires a sign-off on the head commit of **every pull request**, whoever opens it. The author,
labels and commit trailers are not read.

- **A branch of this repository** (members and agents): a successful `local-ci` status. Our Mac runner sets
  it after `bin/ci --quick` (`ci-remote-sweep` starts that run for each such PR), or you set it with
  `bin/ci --signoff`. When it is missing the gate says "Run bin/ci --quick and bin/ci --signoff on this head
  commit."
- **A fork** (community): fork code never runs on our Macs. The sign-off is a successful `CI: Quick` run
  (`ci-quick.yml`, which runs `bin/ci --quick`) on the head commit in GitHub Actions: a `pull_request` job
  with a read-only token and no secrets. A first-time contributor's run needs a maintainer to approve it.

The `agent` label is only for filtering; it does not change the gate.

The requirement is **report-only** at first: the gate prints that line as a warning and passes. Setting the
repository variable `REQUIRE_LOCAL_CI` to `true` makes it blocking. It is the only switch. When blocking, the
gate polls for the status for five minutes, so signing off right after the push is enough; after a failure,
sign off and rerun the gate job.

How it authenticates: `bin/ci --signoff` calls `gh api` with whatever credential `gh` uses on that machine
(`gh auth login` or `GH_TOKEN`), which needs permission to write commit statuses on the repository. It
reads the result of the last green run from `.git/bin-ci/last-run.json` in that checkout, so run `bin/ci`
and `bin/ci --signoff` in the same checkout. The status is an ordinary commit status (context `local-ci`)
on a commit SHA, so another machine or credential can set it, as long as the checks passed on that commit.

## Quick stage

[`ci-quick.yml`](../../.github/workflows/ci-quick.yml) runs `bin/ci --quick` on the pull request: formatting,
lockfiles, SPDX headers, workflow lint and the other checks that build nothing. It is the command you run
locally, so the two cannot drift (`bin/ci --verify-sync` covers the rest). It needs no secrets and builds
nothing, so it is deterministic. A check that needs a package's `node_modules` is skipped there; that
package's own workflow still runs it.

The heavy workflows (Rust, macOS and Swift, Nix, containers, SDK and end-to-end) start with a `quick` job
that calls it, and every job in them waits for it (`needs: quick`). A pull request that fails a cheap check
stops in about a minute instead of spending runner minutes on builds. The jobs use
`if: ${{ !failure() && !cancelled() }}` so they still run on pushes and manual runs, where `quick` is
skipped. `ci-quick.yml` also runs on its own on every pull request, so the gate sees a quick failure even
when no heavy workflow starts. A failed `quick` fails its workflow run; the jobs it skipped are not read as
passed. It runs once per commit: the first `quick` that passes saves a cache marker (`ci-quick-pass-<sha>`) and the
others restore it and finish in seconds. Jobs that start before the marker exists run the checks themselves.
When you add a heavy workflow, add the `quick` job and the `needs` the same way.

The stage also decides whether the calling workflow runs at all (its `scope` job; see the next section). Every
job of a heavy workflow lists `quick` in its `needs` and adds `needs.quick.outputs.skip != 'true'` to its `if`.
`test_ci_gate_workflow.py` checks both.

## Path-scoped gate and checks

One decision says which workflows a pull request needs, and it is used twice:

- **The gate** waits only for the workflows the pull request needs.
- **The workflows** skip their jobs when they are not needed. Each heavy workflow's `quick` stage runs the same
  script with `--workflow <its file>` (the `scope` job in `ci-quick.yml`) and the heavy jobs skip on
  `skip == 'true'`. A workflow's own `paths:` filter is only a cheap pre-filter that stops it from starting
  at all; it must stay a superset of what the map requires.

A skipped workflow is one the gate does not wait for, so a lane skipped by scope never blocks the merge. If the
`scope` job itself fails, the workflow's jobs are skipped and its run fails, so the gate fails.

Two inputs decide the areas:

- The area map, [`.github/ci-areas.json`](../../.github/ci-areas.json): path globs to areas, and areas to workflows.
- The package graph, [`.github/scripts/ci_graph.py`](../../.github/scripts/ci_graph.py): every crate and
  JavaScript package in the repo and their `path` / `workspace:` dependencies, read from the manifests
  (`Cargo.toml`, `package.json`, `pnpm-workspace.yaml`). A test checks it against `cargo metadata`.

The scope step in `.github/workflows/ci-gate.yml` runs
[`.github/scripts/ci_gate_scope.py`](../../.github/scripts/ci_gate_scope.py) on the pull request's file list.

Rules, in order:

1. A changed file that matches a **shared** glob makes the gate wait for every run.
2. A **workspace file** (a Cargo or pnpm workspace's root manifest, `Cargo.lock`, `pnpm-lock.yaml`,
   `rust-toolchain.toml` or `.cargo/`) affects every member of that workspace.
3. A file inside a package (the nearest `Cargo.toml` or `package.json` above it) affects that package.
   A file listed under `inputs` in the map (for example `libs/cua/proto/**` for `cua-proto`) affects the
   packages named there, and its own area.
4. Every affected package also affects **every package that depends on it**, transitively. So a change to a
   shared crate such as `cua-proto` or `cua-driver-core` waits for the areas of all its dependents, and no others.
5. Any other file belongs to the first area (top to bottom in the table) whose globs match it. A file, or an
   affected package, outside every area makes the gate wait for every run.
6. A workflow listed under an area is required only when one of its areas is affected.
7. A workflow listed under no area (for example the eMudhra and release-metadata checks) is always required. A new workflow is therefore required until someone maps it. The gate never waits for **CI: CLA**, because the required `contributor-attribution` check enforces the CLA.
8. A workflow under **`narrow`** in the map is required only when one of the packages it builds (`packages`,
   package directories) is affected under rule 4, or a changed file matches its `paths`. This is the
   "affected" model of Nx and Turborepo: name the packages a suite builds, and the package graph adds
   everything they depend on. Narrowing only removes a workflow that rules 6 and 7 required; a shared file
   still requires it. For example, a change to `cua-spaces-app-core` runs the Rust, web and app lanes that build
   it or its dependents, but not `CI: Test Python` (which builds only `cua-sdk`) or the openkoalabot examples.

`ci-gate` is the only required check, so nothing else needs a placeholder status. Main pushes, nightly and
manual runs are not scoped: `quick` is skipped there, so each workflow runs in full on its own `push` and
`schedule` triggers.

**A pull request cannot narrow its own gate.** The gate checks out the map and both scripts from the base
commit, not from the pull request. It reads the pull request's manifests only to add the dependency edges the
pull request introduces. Any `.github/**` change (workflows, the map, the scripts) waits for every workflow,
and `.github/` changes need a code owner's review.

If the scope step fails for any reason, the gate fails. It never skips on error.

### Merge queue

Every workflow with the `quick` stage also starts on `merge_group`. In the queue the scope is the diff between
the queue's base and the queue commit, so the lanes that the queued pull requests touch run on the merged
result. That is cheaper than running everything, and it is the same rule as a pull request, applied to what
will land on `main`. In a `merge_group` run, `github.event_name` is not `pull_request`, so those lanes run their
`main` jobs too.

This is off until the repository variable `CI_QUEUE_RUNS` is `true`. While it is off, `merge_group` runs skip
every job and the gate in the queue re-reads the pull request's own results, as before. When it is on, the
gate judges a workflow by its queue run where it has one (with at least one job not skipped) and by the pull
request's run otherwise. Turn it on after one queue entry has gone through green with it on.

### When a rerun turns green

The gate is one job that gives one verdict after every other workflow finishes. If a job fails, the gate fails,
at once: it does not wait for the rest of that job's run (unless the failure is a quarantined flake). In the merge
queue this drops a failing entry within about a minute, so the entries behind it are not held until the queue's
60-minute timeout.
[`ci-gate-recheck.yml`](../../.github/workflows/ci-gate-recheck.yml) reruns the gate when a workflow it waits for
completes green (a first run or a rerun) or the `local-ci` sign-off turns green, if the newest gate run for that
commit failed before that result arrived and the commit is still the head of an open pull request without a
hold label. So rerunning a failed job is enough: the gate turns green by itself when everything is green. The
rerun keeps the check name `ci-gate`, so branch protection and auto-merge are unchanged. The recheck's list of
workflows must name every workflow the gate waits for; `test_ci_gate_recheck.py` fails when one is missing.

To see what a change affects locally: `printf '%s\n' <files> | python3 .github/scripts/ci_gate_scope.py`.
To list a package's dependents: `python3 .github/scripts/ci_graph.py cua-proto`.

### Shared files (wait for everything)

`.github/**`, `*` (any file at the repo root), `**/uv.lock` (the Python graph is not modelled yet),
`scripts/ci/**`, `flake.nix`, `nix/**`

Any file outside every area below also counts as shared.

### Areas

| Area | Paths | Workflows mapped | Slow jobs that gate it |
|---|---|---|---|
| `relay` | `libs/cua-spacesd/crates/cua-relay/**` | 5 (narrowed by hand) | `ci-cua-relay-image`, `ci-cua-spacesd` |
| `spacesd` | `libs/cua-spacesd/**` | 19 | `cd-image-linux`, `ci-cua-oci-spacesd`, `ci-cua-relay-image`, `ci-cua-sandbox-doctor`, `ci-cua-sdk`, `ci-cua-spacesd`, `ci-openkoalabot-example-tauri`, `ci-test-python`, `e2e-cua-sdk` |
| `driver-rust` | `libs/cua-driver/rust/**` | 30 | `ci-check-docs`, `ci-cua-sandbox-doctor`, `ci-cua-spacesd`, `ci-rust-linux`, `ci-rust-macos`, `ci-rust-windows`, `e2e-cua-sdk`, `e2e-rust-linux` |
| `driver` | `libs/cua-driver/**`, `libs/cua-driver-bench/**` | 35 | `ci-check-docs`, `ci-cua-sandbox-doctor`, `ci-cua-spacesd`, `ci-rust-linux`, `ci-rust-macos`, `ci-rust-windows`, `e2e-cua-sdk`, `e2e-rust-linux` |
| `cua` | `libs/cua/**`, `tests/e2e/**` | 44 | `cd-image-linux`, `ci-check-docs`, `ci-cua-oci-spacesd`, `ci-cua-relay-image`, `ci-cua-sandbox-doctor`, `ci-cua-sdk`, `ci-cua-spaces-macos`, `ci-cua-spaces-web`, `ci-cua-spaces`, `ci-cua-spacesd`, `ci-openkoalabot-example-tauri`, `ci-test-python`, `e2b-bindings`, `e2e-cua-sdk` |
| `images` | `libs/images/**`, `scripts/images/**` | 19 | `cd-image-linux`, `ci-check-docs`, `ci-cua-spaces-macos`, `ci-cua-spacesd`, `e2e-cua-sdk` |
| `spaces-web` | `apps/cua-spaces-web/**`, `apps/cua-spaces-desktop/**` | 7 | `ci-cua-spaces-macos`, `ci-cua-spaces-web` |
| `spaces-app` | `apps/cua-spaces/**` | 7 | `ci-cua-spaces-web`, `ci-cua-spaces` |
| `spaces-macos` | `apps/cua-spaces-macos/**`, `libs/spaces-app-swift/**`, `libs/spaces-notch-swift/**` | 8 | `ci-cua-spaces-macos`, `ci-cua-spaces-web` |
| `python` | `libs/python/**`, `libs/cua-bench/**`, `libs/cua-bench-rl/**`, `libs/cua-s1/**` | 24 | `ci-check-docs`, `ci-cua-sdk`, `ci-test-python`, `e2e-cua-sdk` |
| `typescript` | `libs/typescript/**`, `libs/cuabot/**` | 9 | none of the slow jobs |
| `lume` | `libs/lume/**`, `libs/lumier/**`, `libs/spaces-sdk-swift/**` | 11 | `ci-check-docs`, `ci-cua-sdk`, `ci-cua-spaces-macos`, `ci-swift-lume` |
| `containers` | `libs/kasm/**`, `libs/qemu-docker/**`, `libs/xfce/**`, `libs/xfce-cua/**` | 11 | none of the slow jobs |
| `docs` | `docs/**`, `skills/**`, `scripts/docs-generators/**` | 17 | `ci-check-docs`, `e2e-cua-sdk` |
| `compat` | `tests/compat/**` | 11 | none of the slow jobs |
| `installers` | `scripts/install/**` | 6 | none of the slow jobs |
| `examples` | `examples/**` | 5 | `ci-openkoalabot-example-tauri` |

The `narrow` entries carry a `why`. The `relay` area is also narrowed by hand, to `ci-cua-relay-image`, `ci-cua-spacesd` and the cheap repo checks.
`cua-spacesd` and `cua-spacesd-server` link `cua-relay`, so the package graph also adds the `spacesd` area
to a relay change. Every other area lists every workflow whose own `paths:` reach it.

### Keeping the map correct

`.github/scripts/tests/test_ci_gate_scope.py` (run by `CI: Test Scripts`) fails when a workflow's
`pull_request` paths reach an area that does not list it, or when a `narrow` entry names a directory that is not
a package. When you add a workflow with a `paths:` filter, or add a path to one, add it to the matching areas in
`.github/ci-areas.json`. When a narrowed workflow starts building another package, add it to its `packages`. Changing the map or the gate
is a `.github/**` change, so it waits for every workflow, and it takes effect only once it is on `main`.

## Heavy bundles run after merge

The installer bundles take 55 to 100 minutes, so a pull request does not wait for them:

- `CI: Cua Spaces` `bundle` (Tauri: Windows NSIS and MSI, macOS DMG, Linux deb and AppImage) builds on every push to
  `main` that touches the app or `libs/cua`. A pull request builds it only when it changes a packaging path and
  carries the `ci:full` label.
- `CI: Cua Spaces for macOS (SwiftUI)` `SwiftUI app (bundle, launch check, XCUITest)` runs on every push to `main`
  and on a manual run. A pull request runs it only when it changes a packaging path: `apps/cua-spaces-macos`
  `scripts/`, `Support/`, `UITests/`, `project.yml`, the Swift package manifests, the pinned driver files, the
  `libs/spaces-app-swift/scripts/` staging script, or the workflow itself (the `changes` job).
- Pull requests keep the 2 to 5 minute installer smokes (`CI: Installers`, the desktop install smoke) and the
  core, binding and Swift test jobs. A release still builds every bundle.
- The gate waits only for runs and jobs that exist on the pull request, so a job that the pull request does not
  start (a skipped `if`, or a trigger that is `push` only) is never waited for. No change to the area map is needed.

A red run on `main` is already merged, so it is fixed forward. It alerts through `workflow-failure-alerts.yml`
(Alertmanager, `#alerts-staging`) and `ci-postmerge-issue-bundles.yml` opens one issue per workflow, labelled
`ci-postmerge`, comments on it for each later failure and closes it on the next green run.

## Jobs that boot a Linux VM need `setup-pasta`

Secure Spaces is always on, so every Linux QEMU guest runs inside a pasta network namespace. Hosted Ubuntu 24.04 runners block the user namespace pasta needs through AppArmor (`Couldn't write to /proc/self/uid_map`), and the VM then fails to start with a misleading IPv6 error.

Add `- uses: ./.github/actions/setup-pasta` to every job that boots a Linux VM or container through cua (qemu, KVM, `cua run`, image builds, bake-world, doctors). Put it after checkout and before the first VM starts. It installs passt through the cached `apt-install` action, adds the `cua-pasta` AppArmor profile and smoke-tests `pasta --config-net -q -- true`. Do not copy the profile inline into a workflow.

## Flaky checks

Known flakes are listed in [`.github/flaky/quarantine.json`](../../.github/flaky/quarantine.json) and
[`.github/workflows/FLAKY.md`](../../.github/workflows/FLAKY.md). When every failed job of a workflow run
matches a `job` entry and its log contains the entry's `signature`, the gate reruns the failed jobs once. If
they pass, the run passes. If they fail again the same way, the gate reports a quarantined flake and does not
fail. The gate reads the list from the base commit. Every entry has an owner, an issue and an expiry date.
The nightly **Flaky job scan** files a `flaky` issue for each job that failed and then passed on a rerun.

## Measuring CI speed

- **Per-file test times.** Jobs that run under `flaky.py` (nextest JUnit, vitest, Swift logs) and the pytest and
  `swift test` steps of `ci-test-python`, `ci-py-bench`, `ci-cua-sdk` and `ci-cua-microvm` write a "Slowest test
  files" table to the job summary and upload `timings-<suite>` (10 days) through
  `.github/scripts/flaky/timings.py`. pytest also prints `--durations=25`. A new suite gets this by calling
  `flaky.sh run` plus the `flaky-upload` action, or `timings.py` plus `ci-timings-upload`.
- **Weekly report.** `ci-weekly-report.yml` rewrites ONE issue, "CI speed this week", every Monday
  (`.github/scripts/ci_weekly_report.py`): push to green, green to merged, the `ci-gate` tail split into
  "waiting for the `local-ci` sign-off" and "other", release-PR wait, CD time per workflow, and the slowest
  test files. Run it by hand with `gh workflow run ci-weekly-report.yml`.
- **The gate tail.** After the last other workflow finishes, `ci-gate` needs one confirming listing (10 s once
  everything is complete) and then the `local-ci` sign-off, polled every 15 s for up to 75 minutes. A long tail is
  almost always the sign-off queue on the Mac runner or a quarantined-flake rerun, not a poll interval; the
  weekly report shows which.

## Urgent-fix lane

Label a pull request `urgent` for a production fix that cannot wait for the full suite:

- The gate still scopes to the changed areas (the same logic as above). The lane skips nothing else.
- The pull request description must reference a Linear issue (`CUA-123`, `CLE-45` or a Linear issue URL) or an
  incident (`Incident #482`, `incident 2026-10-25 relay outage`). The gate fails the pull request without one,
  so it blocks the merge. Edit the description to fix it; the gate re-runs on `edited`.
- `trycua/cua` has no merge queue, so an urgent pull request merges with `gh pr merge --auto --squash` as soon
  as its scoped gate is green. Nothing is waiting in front of it.
- Do not use the lane to skip a failing check. A failing required check still blocks the merge.
