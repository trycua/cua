# Development

This file is the contributor map for the Cua monorepo. Each component owns its
detailed setup, build, and test instructions. Start here, then follow the guide
next to the code you plan to change.

## Choose a Component

| Area                         | Main paths                                                  | Toolchain                           | Start here                                                                                                                   |
| ---------------------------- | ----------------------------------------------------------- | ----------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| cua-driver                   | `libs/cua-driver/rust`, `libs/cua-driver/python`            | Rust, platform SDKs, Nix on Linux   | [`libs/cua-driver/README.md`](libs/cua-driver/README.md), [`libs/cua-driver/rust/README.md`](libs/cua-driver/rust/README.md) |
| Python SDKs and services     | `libs/python`, `libs/cua-bench`                             | Python 3.12, uv                     | Package `pyproject.toml` and README                                                                                          |
| TypeScript SDKs              | `libs/typescript`                                           | Node.js, pnpm                       | [`libs/typescript/README.md`](libs/typescript/README.md) and its `package.json` scripts                                      |
| CuaBot                       | `libs/cuabot`                                               | Node.js, pnpm                       | [`libs/cuabot/README.md`](libs/cuabot/README.md)                                                                             |
| Lume                         | `libs/lume`                                                 | Swift, Xcode command-line tools     | [`libs/lume/Development.md`](libs/lume/Development.md)                                                                       |
| Sandbox and container images | `libs/kasm`, `libs/lumier`, `libs/qemu-docker`, `libs/xfce` | Docker plus the component toolchain | README or Development file in the component                                                                                  |
| Public documentation         | `docs/content/docs`                                         | Node.js, pnpm, Fumadocs             | [`docs/README.md`](docs/README.md)                                                                                           |
| Samples                      | `samples`                                                   | Depends on the sample               | README next to the sample                                                                                                    |

The repository changes quickly. Directory listings in this root guide are an
orientation aid, not a package registry. Release targets come from
[`.github/workflows/release-bump-version.yml`](.github/workflows/release-bump-version.yml),
and test ownership comes from CI plus the component guides.

## Common Setup

Clone the repository and enter it:

```bash
git clone https://github.com/trycua/cua.git
cd cua
```

Install only the toolchains required by your component:

| Work                        | Required tools                                                              |
| --------------------------- | --------------------------------------------------------------------------- |
| Python packages             | Python 3.12 and [uv](https://docs.astral.sh/uv/)                            |
| TypeScript, CuaBot, or docs | The Node.js version required by the component and its declared pnpm version |
| cua-driver                  | Rust plus the target OS SDK; Nix for the reproducible Linux lanes           |
| Lume                        | macOS, Swift, and Xcode command-line tools                                  |
| Images                      | Docker or the image-specific builder documented by the component            |

The root uv workspace contains only the members declared in
[`pyproject.toml`](pyproject.toml). Other directories under `libs/python` are
independent packages; use their own `pyproject.toml` and CI workflow.

The root Node package installs repository-wide Prettier only. Run `pnpm
install` inside `libs/typescript`, `libs/cuabot`, or `docs` for those
components' dependencies.

API keys are not required for ordinary builds and deterministic tests. Add
credentials only for a test or example that explicitly calls an external
provider, and never commit them.

## Root Formatting Hooks

Install the root Python development tools and optional Git hooks with:

```bash
uv sync --group dev
uv run pre-commit install
```

Run all configured hooks against the repository with:

```bash
uv run pre-commit run --all-files
```

The hooks currently run Prettier, the TypeScript workspace typecheck, Black,
isort, and Ruff. Install the TypeScript workspace dependencies before running
the typecheck hook. Mypy is configured in `pyproject.toml` but is not a
pre-commit gate.

For a read-only repository-wide formatting check:

```bash
pnpm install --frozen-lockfile
pnpm prettier:check
```

Component-specific Rust, TypeScript, Swift, and documentation checks remain in
their component guides. See [`TESTING.md`](TESTING.md) for the test map.

## cua-driver Development

cua-driver has three distinct validation layers:

1. Rust unit and protocol tests that do not require a target GUI application.
2. Source-built harness E2E tests that drive Electron, Tauri, and native toolkit
   fixtures in a real user desktop session.
3. Optional real-application checks for software that is not part of the
   canonical harness catalog.

The Rust harnesses are the source of truth for desktop behavior. Python tests
do not duplicate that matrix. Start with:

- [`libs/cua-driver/rust/README.md`](libs/cua-driver/rust/README.md) for the Cargo workspace.
- [`libs/cua-driver/rust/crates/cua-driver/tests/README.md`](libs/cua-driver/rust/crates/cua-driver/tests/README.md) for test ownership.
- [`scripts/ci/README.md`](scripts/ci/README.md) for canonical OS runners.
- [Platform support](https://cua.ai/docs/reference/cua-driver/platform-support) for current capability boundaries.
- [How Cua Driver is validated](https://cua.ai/docs/concepts/how-cua-driver-is-validated) for the public evidence model.
- [Platform roadmap](https://cua.ai/docs/reference/cua-driver/platform-roadmap) for remaining work and platform boundaries.

Windows and macOS desktop tests need a real user session. Windows requires an
active console or RDP session. macOS requires a logged-in session with
Accessibility and Screen Recording permissions. The hosted Linux Sway and
nested-compositor runners create controlled sessions; GNOME, KDE, and real
Xorg validation use an existing graphical login.

## Documentation

Public docs use Fumadocs and follow Diataxis:

- tutorials teach a first success;
- how-to guides solve a specific task;
- concepts explain constraints and design;
- reference pages state commands, contracts, support, and limits.

Run documentation commands from `docs`; see [`docs/README.md`](docs/README.md).
Contributor-only implementation notes should remain next to their component
instead of entering the public docs navigation.

## Releases

Maintainers release packages through the
[CD: Bump Version](https://github.com/trycua/cua/actions/workflows/release-bump-version.yml)
workflow. Its `service` input is the current release-target registry. Each
package's `.bumpversion.cfg`, Cargo manifest, or package manifest owns its
version and tag format.

Do not duplicate the complete release-target list or example versions in this
guide. They become stale as components are added. The workflow bumps one target
at a time and tag-triggered CD workflows perform publication.

The root `Makefile` provides local version inspection and dry-run helpers. It
does not publish production releases.

## Generated and Local Files

Do not commit build products, staged harness binaries, local VM artifacts,
credentials, permission databases, or editor-specific state. Promote an
artifact into source control only when it becomes a stable fixture, sample, or
maintained document.
