# CI

Every pull request must pass one required check, **CI: gate** (`ci-gate`). The gate waits for the other
pull request workflows and passes when they pass. This page explains which of those workflows it waits for.

## Path-scoped gate

A workflow still starts on its own `paths:` filter. The gate then waits only for the workflows that belong
to the areas the pull request affects. Slow jobs for other areas (for example the Linux image build on a
docs-only change) still run, but they no longer hold up the merge.

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

Unaffected workflows still run if their own `paths:` match; the gate just doesn't wait for them. `ci-gate`
is the only required check, so nothing else needs a placeholder status. Main pushes, nightly and manual runs
are not scoped: each workflow runs in full on its own `push` and `schedule` triggers.

**A pull request cannot narrow its own gate.** The gate checks out the map and both scripts from the base
commit, not from the pull request. It reads the pull request's manifests only to add the dependency edges the
pull request introduces. Any `.github/**` change (workflows, the map, the scripts) waits for every workflow,
and `.github/` changes need a code owner's review.

Merge-queue runs (`merge_group`) read the file list of the same pull request through the same API call, so
the queue and the pull request wait on the same set. If the scope step fails for any reason, the gate fails.
It never skips on error.

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
| `spaces-macos` | `apps/cua-spaces-macos/**` | 7 | `ci-cua-spaces-macos`, `ci-cua-spaces-web` |
| `python` | `libs/python/**`, `libs/cua-bench/**`, `libs/cua-bench-rl/**`, `libs/cua-s1/**` | 24 | `ci-check-docs`, `ci-cua-sdk`, `ci-test-python`, `e2e-cua-sdk` |
| `typescript` | `libs/typescript/**`, `libs/cuabot/**` | 9 | none of the slow jobs |
| `lume` | `libs/lume/**`, `libs/lumier/**`, `libs/spaces-sdk-swift/**` | 11 | `ci-check-docs`, `ci-cua-sdk`, `ci-cua-spaces-macos`, `ci-swift-lume` |
| `containers` | `libs/kasm/**`, `libs/qemu-docker/**`, `libs/xfce/**`, `libs/xfce-cua/**` | 11 | none of the slow jobs |
| `docs` | `docs/**`, `skills/**` | 17 | `ci-check-docs`, `e2e-cua-sdk` |
| `compat` | `tests/compat/**` | 11 | none of the slow jobs |
| `installers` | `scripts/install/**` | 6 | none of the slow jobs |
| `examples` | `examples/**` | 5 | `ci-openkoalabot-example-tauri` |

The `relay` area is narrowed by hand to `ci-cua-relay-image`, `ci-cua-spacesd` and the cheap repo checks.
`cua-spacesd` and `cua-spacesd-server` link `cua-relay`, so the package graph also adds the `spacesd` area
to a relay change. Every other area lists every workflow whose own `paths:` reach it.

### Keeping the map correct

`.github/scripts/tests/test_ci_gate_scope.py` (run by `CI: Test Scripts`) fails when a workflow's
`pull_request` paths reach an area that does not list it. When you add a workflow with a `paths:` filter,
or add a path to one, add it to the matching areas in `.github/ci-areas.json`. Changing the map or the gate
is a `.github/**` change, so it waits for every workflow, and it takes effect only once it is on `main`.

## Urgent-fix lane

Label a pull request `urgent` for a production fix that cannot wait for the full suite:

- The gate still scopes to the changed areas (the same logic as above). The lane skips nothing else.
- The pull request description must reference a Linear issue (`CUA-123`, `CLE-45` or a Linear issue URL) or an
  incident (`Incident #482`, `incident 2026-10-25 relay outage`). The gate fails the pull request without one,
  so it blocks the merge. Edit the description to fix it; the gate re-runs on `edited`.
- `trycua/cua` has no merge queue, so an urgent pull request merges with `gh pr merge --auto --squash` as soon
  as its scoped gate is green. Nothing is waiting in front of it.
- Do not use the lane to skip a failing check. A failing required check still blocks the merge.
