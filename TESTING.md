# Testing

Cua is a multi-language monorepo. There is no single root command that proves
every package, desktop, VM, and image. Run the tests owned by the components you
changed and use the corresponding CI workflow as the executable source of
truth.

## Test Map

| Area                         | Deterministic tests                                        | Integration or E2E owner                                                          |
| ---------------------------- | ---------------------------------------------------------- | --------------------------------------------------------------------------------- |
| cua SDK and CLI (`libs/cua`) | Cargo tests, proto lint/breaking, binding drift, per-language smoke tests | `tests/e2e/cua-sdk` (**E2E: cua SDK**)                               |
| cua-spacesd               | Cargo tests and the in-process conformance suite           | Linux core tests and relay E2E in Docker                                          |
| Python SDKs                  | Package `tests/` directories with pytest                   | Package-specific integration tests and `tests/integration`                        |
| cua-sandbox                  | Hermetic pytest suite on the cua SDK                       | **Periodic: Cua Sandbox Live Fleet E2E**                                          |
| TypeScript SDKs              | Package Vitest/typecheck scripts                           | Package-owned integration tests                                                   |
| cua-driver                   | Rust unit, schema, protocol, and compile tests             | Canonical Rust desktop harnesses on Windows, macOS, Linux X11, and Linux Wayland  |
| Lume                         | Swift package tests                                        | VM and unattended-setup checks documented by Lume                                 |
| Cua Spaces app               | Vitest, Tauri Rust tests                                   | **CI: Cua Spaces**                                                                |
| Installers                   | `scripts/install/tests`                                    | **CI: Installers** smoke jobs on Linux, macOS, and Windows                        |
| Public docs                  | Generator drift, hygiene, links, and production build      | Rendered Fumadocs site                                                            |
| Images and sandboxes         | Component build and schema tests; **CI: Check Image Refs** (`python3 scripts/images/check-image-refs.py`: every image ref comes from `libs/images/sandbox-images.json`, a bench lock or the allowlist; frozen legacy names only in their listed paths) | **CD: Image linux**, **E2E: Fleet images, local**, image smoke tests  |

Path-filtered CI avoids running unrelated operating systems, so a green job for
one component does not validate another component.

## Python

For a member of the root uv workspace (`libs/python/{agent,core,som,bench-ui}`):

```bash
uv sync --group test
CUA_TELEMETRY_ENABLED=false uv run pytest libs/python/<package>/tests -v
```

Packages outside the root uv workspace (`cua-sandbox`, `cua-train`,
`libs/cua-bench`, `libs/cua/python`) are installed from their own
`pyproject.toml`. cua-sandbox runs on the cua SDK, so build the native library
first and pass explicit ignores: several legacy files start real VMs.

```bash
cd libs/cua
cargo build --locked --release -p cua-sdk
scripts/build-test-fixtures.sh
node scripts/stage-uniffi-library.mjs --only=python
cd ../python/cua-sandbox
uv sync --frozen --python 3.12
CUA_TELEMETRY_ENABLED=false uv run --frozen pytest tests --timeout=120 \
  --ignore=tests/live --ignore=tests/test_runtime.py --ignore=tests/test_snapshots.py \
  --ignore=tests/test_windows_cloud.py --ignore=tests/test_windows_timing.py \
  --deselect tests/test_oci.py::TestLiveRegistry
```

The package matrix and exact commands live in
[`.github/workflows/ci-test-python.yml`](.github/workflows/ci-test-python.yml).

## cua SDK and CLI

Run from `libs/cua` (needs `protoc`):

```bash
cargo test --locked -p cua-sdk -p cua-daemon -p cua-cli -p cua-teleport -p cua-teleport-bundle -- --test-threads=4
cargo test --locked -p cua-proto --all-features
scripts/check-proto.sh                            # buf lint, format, breaking
node scripts/generate-uniffi-bindings.mjs --check # binding drift
scripts/sync-skills.sh --check                    # bundled skills drift
```

Test other crates with `cargo test --locked -p <crate>`; their READMEs name
any opt-in live gates.

Per-language smoke tests (Python, Node, Swift, Kotlin, browser) are in each
package README under `libs/cua/{python,typescript,swift,kotlin}` and in
[`.github/workflows/ci-cua-sdk.yml`](.github/workflows/ci-cua-sdk.yml).

The cross-language E2E suite runs each guide scenario in Python, TypeScript,
Rust, and Go, per lane (`hermetic`, `container`, `qemu`, `lume`, `fleet`,
`fleet-env`, `cua-sandbox`, `conformance`). Lanes other than `hermetic` start
containers, VMs, or Fleet claims and are opt-in:

```bash
tests/e2e/cua-sdk/ci-setup.sh --images
python3 tests/e2e/cua-sdk/run.py --lanes hermetic,container --langs py,ts,rust
```

## cua-spacesd

Run from `libs/cua-spacesd`:

```bash
cargo test --workspace --locked
scripts/ci/linux-core-tests.sh   # Linux container: large transfers, long streams
scripts/ci/relay-e2e.sh          # driver with no published ports behind cua-relay
```

Point the conformance suite at any running driver with
`CUA_ENV_TEST_TARGET=http://host:3211 CUA_ENV_TEST_TOKEN=... cargo test -p cua-spacesd-server --test conformance`.
See [`crates/cua-spacesd-server/README.md`](libs/cua-spacesd/crates/cua-spacesd-server/README.md#tests).

## TypeScript

Run from `libs/typescript`:

```bash
pnpm install --frozen-lockfile
pnpm test
pnpm typecheck
pnpm format:check
```

Use a package's own `package.json` scripts when working outside that workspace,
including CuaBot and the documentation site.

## cua-driver Unit and Protocol Tests

Run from `libs/cua-driver/rust`. Focused examples:

```bash
cargo test -p cua-driver-core --locked
cargo test -p cua-driver --test protocol_schema_test --locked
```

Linux source and package checks run through Nix. Windows and Linux compile gates
are split into OS-specific workflows. See
[`libs/cua-driver/rust/README.md`](libs/cua-driver/rust/README.md) for workspace
commands.

Unit and protocol tests do not prove that desktop input reached a real
application.

## cua-driver Harness E2E

The canonical desktop suites build repository-owned applications, drive them
through the Rust driver, and verify application or desktop state independently
from the tool response. Foreground/background delivery and AX/PX addressing are
dimensions of each action row.

Canonical entry points:

```text
Linux X11/session: scripts/ci/linux/run-rust-e2e.sh
Linux Sway:        scripts/ci/linux/run-rust-e2e-wayland.sh
Linux nested:      scripts/ci/linux/run-rust-e2e-inject.sh
Linux GNOME/KDE:   scripts/ci/linux/run-rust-e2e-desktop.sh <gnome|kde>
Linux real Xorg:   scripts/ci/linux/run-rust-e2e-desktop.sh xorg
Windows:           .\scripts\ci\windows\run-rust-e2e.ps1 -RequireGui
macOS:             scripts/ci/macos/run-rust-e2e.sh
```

The hosted Sway and nested-compositor runners create controlled sessions.
GNOME, KDE, real Xorg, Windows, and macOS use an existing graphical login. The
suites are often maintainer-triggered and retain typed case/results,
screenshots, accessibility state, trajectories, logs, and video where the lane
supports it. The reporter rejects missing rows, false-success responses,
undeclared outcomes, and incomplete required evidence.

See:

- [`libs/cua-driver/docs/test-harnesses-guide.md`](libs/cua-driver/docs/test-harnesses-guide.md)
- [`libs/cua-driver/docs/test-matrix.md`](libs/cua-driver/docs/test-matrix.md)
- [Platform support and validation](https://cua.ai/docs/cua-driver/concepts/platform-support)

## Lume

Run from `libs/lume`:

```bash
swift test
```

VM-dependent and unattended-setup checks have additional prerequisites in
[`libs/lume/Development.md`](libs/lume/Development.md).

## Public Documentation

Run from `docs`:

```bash
pnpm install --frozen-lockfile
pnpm docs:check-hygiene
pnpm docs:check-links
pnpm docs:check-blocks
pnpm build
```

The production build validates MDX compilation and static route generation.
Curated MDX changes do not need a product build. `docs:check-blocks` checks that
every code block declares how it is tested (and runs the static cli-shape and
config lanes); the runnable blocks run with
`python3 tests/e2e/cua-sdk/run.py --lanes docs --langs py --strict`. Generated
reference changes also run the owning generator's check (`pnpm docs:check:<name>`,
listed by `pnpm docs:list`); `pnpm docs:check` is the explicit full audit. See
[`docs/README.md`](docs/README.md) for details.

## Before Opening a Pull Request

1. Run focused tests while developing.
2. Run the complete deterministic test owner for every component changed.
3. Run the affected interactive E2E lane when desktop behavior or its contract changes.
4. Run formatting and documentation checks for modified files.
5. Record any test that could not run and why.

Do not turn missing dependencies, desktop sessions, fixtures, or permissions
into a reduced green run. Environment failures and unsupported capabilities
must remain visible.
