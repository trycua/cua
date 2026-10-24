# CI map: what a change runs, and how the builds are cached

Every workflow here is gated by `paths:` so a pull request only runs what its
files can affect. Before you push, run the cheap gates locally:

    scripts/ci/prepush.sh            # headers, lockfiles, fmt, clippy + Windows cfg for changed crates
    scripts/ci/prepush.sh --fast     # seconds: skip clippy and Windows
    scripts/ci/prepush.sh --bindings # also regenerate and compare the UniFFI bindings

## Which change runs what (pull requests)

Typical wall time is the longest workflow in the row (warm cache). Rows are not
exclusive: a change that touches two areas runs both.

| You changed | Workflows that run |
|---|---|
| `libs/cua/**` (Rust core: SDK, daemon, CLI, spaces crates) | CI: cua SDK (rust lint, rust test, spaces e2e, bindings drift, python/node/browser smoke, kotlin; macOS and Windows legs with the `ci:full` label), CI: Cua Spaces (frontend + Rust tests; installer bundles only with the `ci:full` label), CI: Test Python, CI: openkoalabot-example (TypeScript and Tauri; its Swift job only for the rows below), CI: cua contrib providers, CI: cua your-cloud providers, CI: Check Docs |
| `libs/cua/swift/**`, `libs/spaces-sdk-swift/**`, `libs/spaces-app-swift/**`, `libs/cua/crates/cua-sdk/**`, `libs/cua/proto/**`, the Rust workspace manifest, lockfile and toolchain pin, `.github/actions/setup-protoc/**`, `.github/actions/rust-cache/**`, or the Swift samples (`examples/infinite-canvas-swift/**`, `examples/openkoalabot-example-swift/**` and the files its tests read) | CI: infinite-canvas (Swift) and the Swift job of CI: openkoalabot-example, each 28 to 33 macOS minutes. The full list is in `.github/scripts/ci_swift_samples_scope.py`. Other `libs/cua/**` changes skip them. Both workflows start a cheap scope job on every pull request and push; it decides, so main, the nightly schedule and the `ci:full` label build them every time. Release-please branches skip them as before |
| `libs/cua-spacesd/**` | CI: cua-spacesd (3 OS + relay e2e), CI: Image doctor, CD: Image linux (build and gates only), CI: Images on the current cua-spacesd, openkoalabot (Tauri) |
| `libs/cua-driver/**` | CI: Rust Linux / macOS / Windows unit, CI: Nix Linux Rust source, CI: cua-driver contract clients, driver quick feedback, Image doctor, CI: cua-spacesd |
| `apps/cua-spaces/src/**` (web frontend only) | CI: Cua Spaces: vitest + typecheck and the Rust command-layer tests. The installer bundles (a Tauri app that does not ship) are not built on pull requests; add the `ci:full` label to build them, and then the Windows and macOS bundles run only when something other than `src/`, `public/`, `e2e/`, `tests/`, `docs/` or markdown changed |
| `apps/cua-spaces/src-tauri/**` or anything native | CI: Cua Spaces (Rust command-layer tests compile and test the Tauri crate; all three bundles with the `ci:full` label) |
| `apps/cua-spaces-macos/**` | CI: Cua Spaces for macOS (SwiftUI) |
| `apps/cua-spaces-desktop/**` | CI: Cua Spaces web UI and Electron shell |
| `docs/**` | CI: Check Docs Links, Docs: compile lanes, Docs page tests |
| `libs/python/**` | CI: Test Python, per-package Python workflows |
| `libs/images/**`, `scripts/images/**` | CD: Image linux / omarchy (PR builds gate, nothing is pushed), Check Image Refs |
| `.github/workflows/<x>.yml` | that workflow, plus CI: Lint workflows |
| markdown, comments only | metadata checks only (Release metadata, SPDX, link checks) |

Main runs every workflow above on its own paths too (the Rust ones warm the
build cache, below), and the nightly schedules cover the rest. The Cua Spaces
Tauri installer bundles are the one thing pull requests skip by default: they
build on main pushes, manual runs and `ci:full` pull requests.

## Required checks

Staging has no required status checks. trycua/cua requires `fleet-mirror-guard`,
`contributor-attribution` and `validate`; none of those are renamed or removed
here. A workflow skipped by `paths:` never reports, so do not add a path-gated
check to the required list without a gate job that always runs.

## Build cache (Rust)

All Rust CI jobs use `.github/actions/rust-cache`, which routes `rustc` through
sccache with a local cache directory persisted by `actions/cache` (one
size-capped entry per OS and workspace). Why not `Swatinem/rust-cache`: it
stored a whole `target/` per job (1-3 GB each), always dropped the workspace
crates, and filled the repository's 10 GB cache budget, so entries were evicted
within minutes and most jobs compiled cold. sccache is keyed by the content of
each compile unit, so an unchanged crate is reused in any job of any workflow.

* main saves the cache on every OS (one entry per OS and family; clippy, test
  and build jobs of the same workspace use separate families because they
  compile different units).
* PRs restore main's cache and do not save. After raising the repository's
  Actions cache limit, flip the `pr-save` default in the action and
  Linux PR runs also save, so a PR's next push builds on its previous one.
* Workflows that build the SDK have a `push: main` trigger (same paths) so main
  keeps the cache warm. Their concurrency only cancels superseded pull request
  runs, never main runs (a cancelled run saves nothing).
* CI uses a cheaper release profile (opt-level 1, no LTO, no debug info) for
  smoke tests; release builds in `cd-*.yml` keep the shipped profile.
* When sccache cannot start, cargo compiles normally.

## Known flaky tests and quarantine

See [`FLAKY.md`](FLAKY.md). A test goes on the list with an owner and a fix
date, never as a silent retry.
