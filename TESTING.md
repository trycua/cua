# Testing

Cua is a multi-language monorepo. There is no single root command that proves
every package, desktop, VM, and image. Run the tests owned by the components you
changed and use the corresponding CI workflow as the executable source of
truth.

## Test Map

| Area                 | Deterministic tests                                   | Integration or E2E owner                                                         |
| -------------------- | ----------------------------------------------------- | -------------------------------------------------------------------------------- |
| Python SDKs          | Package `tests/` directories with pytest              | Package-specific integration tests and `tests/integration`                       |
| TypeScript SDKs      | Package Vitest/typecheck scripts                      | Package-owned integration tests                                                  |
| cua-driver           | Rust unit, schema, protocol, and compile tests        | Canonical Rust desktop harnesses on Windows, macOS, Linux X11, and Linux Wayland |
| Lume                 | Swift package tests                                   | VM and unattended-setup checks documented by Lume                                |
| Public docs          | Generator drift, hygiene, links, and production build | Rendered Fumadocs site                                                           |
| Images and sandboxes | Component build and schema tests                      | Image-specific smoke or VM tests                                                 |

Path-filtered CI avoids running unrelated operating systems, so a green job for
one component does not validate another component.

## Python

For a member of the root uv workspace:

```bash
uv sync --group test
CUA_TELEMETRY_ENABLED=false uv run pytest libs/python/<package>/tests -v
```

Packages outside the root uv workspace should be installed from their own
`pyproject.toml`. The current package matrix and installation sequence live in
[`.github/workflows/ci-test-python.yml`](.github/workflows/ci-test-python.yml).

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
cargo test -p cua-driver --test protocol_mcp_test --locked
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
- [Platform support and validation](https://cua.ai/docs/reference/cua-driver/platform-support)

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
pnpm docs:check
pnpm docs:check-hygiene
pnpm docs:check-links
pnpm build
```

The production build validates MDX compilation and static route generation.
The generator check prevents generated CLI and API references from drifting
from source.

## Before Opening a Pull Request

1. Run focused tests while developing.
2. Run the complete deterministic test owner for every component changed.
3. Run the affected interactive E2E lane when desktop behavior or its contract changes.
4. Run formatting and documentation checks for modified files.
5. Record any test that could not run and why.

Do not turn missing dependencies, desktop sessions, fixtures, or permissions
into a reduced green run. Environment failures and unsupported capabilities
must remain visible.
